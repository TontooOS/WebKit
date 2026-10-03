/*
 * tontoo_wpe.c -- WPE WebKit offscreen engine for TontooWebKit.
 *
 * How the offscreen path works (WebKit 2.52 / WPE 1.16):
 *
 *   1. We create a surfaceless EGL display. Nothing of ours needs a
 *      window system, so this works in a container, over SSH and inside
 *      a compositor session alike.
 *   2. `wpe_fdo_initialize_for_egl_display()` hands that display to the
 *      FDO backend, so `wpe_view_backend_create()` no longer needs X11
 *      or Wayland.
 *   3. `wpe_view_backend_exportable_fdo_egl_create()` gives us a view
 *      backend that exports every rendered frame as a wl_shm buffer
 *      instead of scanning out to a native window.
 *   4. WebKit renders into it; we copy the SHM pixels into BGRA and the
 *      UI uploads them as a texture.
 *
 * No frame is ever copied twice: the exportable callback copies once
 * into the owned frame buffer and the Rust side takes that buffer.
 */
#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <glib.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <wpe/wpe.h>
#include <wpe/fdo.h>
#include <wpe/fdo-egl.h>
#include <wpe/webkit.h>
#include <libsoup/soup-cookie.h>
#include <wayland-server-protocol.h>

#include "tontoo_wpe.h"

#define EVENT_RING 512
#define TEXT_MAX 1024

struct TontooWpe {
    WebKitWebView *view;
    struct wpe_view_backend *backend;
    struct wpe_view_backend_exportable_fdo *exportable;
    struct wpe_view_backend_client client;

    /* Newest frame, owned. BGRA. */
    uint8_t *frame;
    uint32_t frame_w;
    uint32_t frame_h;
    uint64_t frame_seq;

    /* Event ring: callbacks only write here. */
    TontooWpeEvent ring[EVENT_RING];
    int ring_head;
    int ring_tail;

    /* Pending JavaScript correlations. */
    struct {
        int64_t id;
        char *script;
    } pending[64];
    int pending_count;
    int64_t next_dialog_id;
    int64_t next_download_id;

    /* Latest answers to synchronously readable questions. */
    char url[TEXT_MAX];
    char title[TEXT_MAX];
    double progress;
    int ready_sent;
};

static pthread_once_t init_once = PTHREAD_ONCE_INIT;
static char init_error[TEXT_MAX];
static EGLDisplay g_egl_display;

static void fail_init(const char *message)
{
    snprintf(init_error, sizeof(init_error), "%s", message);
}

/* ---------------------------------------------------------------------- */
/* one-time process setup                                                 */
/* ---------------------------------------------------------------------- */

static void process_init(void)
{
    PFNEGLGETPLATFORMDISPLAYEXTPROC get_platform_display =
        (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT");
    if (!get_platform_display)
        get_platform_display = (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetPlatformDisplay;

    g_egl_display = get_platform_display(EGL_PLATFORM_SURFACELESS_MESA,
                                         EGL_DEFAULT_DISPLAY, NULL);
    if (g_egl_display == EGL_NO_DISPLAY) {
        fail_init("no surfaceless EGL display");
        return;
    }
    EGLint major, minor;
    if (!eglInitialize(g_egl_display, &major, &minor)) {
        fail_init("eglInitialize failed");
        return;
    }
    if (!eglBindAPI(EGL_OPENGL_ES_API)) {
        fail_init("no EGL_OPENGL_ES_API");
        return;
    }
    if (!wpe_loader_init("libWPEBackend-fdo-1.0.so")) {
        fail_init("wpe_loader_init(libWPEBackend-fdo-1.0.so) failed");
        return;
    }
    if (!wpe_fdo_initialize_for_egl_display(g_egl_display)) {
        fail_init("wpe_fdo_initialize_for_egl_display failed "
                  "(is wpebackend-fdo installed?)");
        return;
    }
}

const char *tontoo_wpe_version(void)
{
    static char version[TEXT_MAX];
    static int done;
    if (done)
        return version[0] ? version : NULL;
    done = 1;
    if (pthread_once(&init_once, process_init) != 0 || init_error[0]) {
        version[0] = '\0';
        return NULL;
    }
    snprintf(version, sizeof(version), "wpe-webkit %u.%u.%u, wpe %u.%u.%u",
             webkit_get_major_version(), webkit_get_minor_version(),
             webkit_get_micro_version(), wpe_get_major_version(),
             wpe_get_minor_version(), wpe_get_micro_version());
    return version;
}

const char *tontoo_wpe_backend_info(void)
{
    static char info[TEXT_MAX];
    static int done;
    if (!done) {
        done = 1;
        snprintf(info, sizeof(info), "EGL %s / %s",
                 (const char *)eglQueryString(g_egl_display, EGL_VENDOR),
                 (const char *)eglQueryString(g_egl_display, EGL_VERSION));
    }
    return info;
}

/* ---------------------------------------------------------------------- */
/* event ring                                                              */
/* ---------------------------------------------------------------------- */

static TontooWpeEvent *event_slot(TontooWpe *self)
{
    int next = (self->ring_head + 1) % EVENT_RING;
    if (next == self->ring_tail)
        return NULL; /* full: drop, the UI polls state anyway */
    TontooWpeEvent *event = &self->ring[self->ring_head];
    self->ring_head = next;
    memset(event, 0, sizeof(*event));
    return event;
}

static void emit_text(TontooWpe *self, int kind, const char *text)
{
    if (!text)
        return;
    TontooWpeEvent *event = event_slot(self);
    if (!event)
        return;
    event->kind = kind;
    snprintf(event->text, TEXT_MAX, "%s", text);
}

static void emit_number(TontooWpe *self, int kind, double number)
{
    TontooWpeEvent *event = event_slot(self);
    if (!event)
        return;
    event->kind = kind;
    event->number = (int64_t)(number * 1000000.0);
}

static void emit_text_id(TontooWpe *self, int kind, int64_t id, const char *text)
{
    TontooWpeEvent *event = event_slot(self);
    if (!event)
        return;
    event->kind = kind;
    event->id = id;
    snprintf(event->text, TEXT_MAX, "%s", text ? text : "");
}

static void emit_number_id(TontooWpe *self, int kind, int64_t id, int64_t number)
{
    TontooWpeEvent *event = event_slot(self);
    if (!event)
        return;
    event->kind = kind;
    event->id = id;
    event->number = number;
}

int tontoo_wpe_next_event(TontooWpe *self, TontooWpeEvent *out)
{
    if (!self || !out || self->ring_tail == self->ring_head)
        return 0;
    *out = self->ring[self->ring_tail];
    self->ring_tail = (self->ring_tail + 1) % EVENT_RING;
    return 1;
}

/* ---------------------------------------------------------------------- */
/* frames                                                                  */
/* ---------------------------------------------------------------------- */

static void on_shm_buffer(void *data, struct wpe_fdo_shm_exported_buffer *buffer)
{
    TontooWpe *self = data;
    struct wl_shm_buffer *shm = wpe_fdo_shm_exported_buffer_get_shm_buffer(buffer);
    if (!shm)
        return;
    wl_shm_buffer_begin_access(shm);
    int32_t width = wl_shm_buffer_get_width(shm);
    int32_t height = wl_shm_buffer_get_height(shm);
    int32_t stride = wl_shm_buffer_get_stride(shm);
    uint32_t format = wl_shm_buffer_get_format(shm);
    const uint8_t *src = wl_shm_buffer_get_data(shm);
    /* ARGB8888 / XRGB8888 only: one 32 bit little endian pixel per word. */
    if (src && width > 0 && height > 0
        && (format == WL_SHM_FORMAT_ARGB8888 || format == WL_SHM_FORMAT_XRGB8888)) {
        size_t needed = (size_t)width * (size_t)height * 4;
        if (!self->frame || self->frame_w != (uint32_t)width
            || self->frame_h != (uint32_t)height) {
            uint8_t *fresh = realloc(self->frame, needed);
            if (!fresh) {
                wl_shm_buffer_end_access(shm);
                return;
            }
            self->frame = fresh;
            self->frame_w = (uint32_t)width;
            self->frame_h = (uint32_t)height;
        }
        for (int32_t y = 0; y < height; y++) {
            const uint8_t *in = src + (size_t)y * (size_t)stride;
            uint8_t *out = self->frame + (size_t)y * (size_t)width * 4;
            for (int32_t x = 0; x < width; x++) {
                uint32_t pixel;
                memcpy(&pixel, in + (size_t)x * 4, 4);
                /* memory is 0xAARRGGBB -> the UI wants 0xBBGGRRAA */
                out[x * 4 + 0] = (uint8_t)((pixel >> 16) & 0xff);
                out[x * 4 + 1] = (uint8_t)((pixel >> 8) & 0xff);
                out[x * 4 + 2] = (uint8_t)(pixel & 0xff);
                out[x * 4 + 3] = 0xff;
            }
        }
        self->frame_seq++;
    }
    wl_shm_buffer_end_access(shm);
    if (!self->ready_sent) {
        self->ready_sent = 1;
        emit_text(self, TONTOO_WPE_READY, "ready");
    }
}

static void on_egl_image(void *data, void *image)
{
    (void)data;
    (void)image;
}

static void on_fdo_egl_image(void *data, struct wpe_fdo_egl_exported_image *image)
{
    (void)data;
    (void)image;
}

static void on_set_size(void *data, uint32_t width, uint32_t height)
{
    (void)data;
    (void)width;
    (void)height;
}

static void on_frame_displayed(void *data)
{
    /* The SHM callback above already copied the pixels. */
    (void)data;
}

static void on_activity_state_changed(void *data, uint32_t state)
{
    (void)data;
    (void)state;
}

static void *on_get_accessible(void *data)
{
    (void)data;
    return NULL;
}

static void on_device_scale_factor(void *data, float scale)
{
    (void)data;
    (void)scale;
}

static void on_target_refresh_rate(void *data, uint32_t rate)
{
    (void)data;
    (void)rate;
}

int tontoo_wpe_take_frame(TontooWpe *self, uint8_t **bgra, uint32_t *w, uint32_t *h,
                          uint64_t *seq)
{
    if (!self || !bgra)
        return 0;
    if (self->frame_seq == 0 || !self->frame)
        return 0;
    size_t size = (size_t)self->frame_w * (size_t)self->frame_h * 4;
    uint8_t *copy = malloc(size);
    if (!copy)
        return 0;
    memcpy(copy, self->frame, size);
    *bgra = copy;
    *w = self->frame_w;
    *h = self->frame_h;
    *seq = self->frame_seq;
    return 1;
}

void tontoo_wpe_free_buffer(uint8_t *buffer)
{
    free(buffer);
}

/* ---------------------------------------------------------------------- */
/* web view signals                                                        */
/* ---------------------------------------------------------------------- */

static void copy_field(char *dst, const char *src)
{
    snprintf(dst, TEXT_MAX, "%s", src ? src : "");
}

static void on_notify_title(GObject *object, GParamSpec *spec, gpointer data)
{
    (void)spec;
    TontooWpe *self = data;
    copy_field(self->title, webkit_web_view_get_title(WEBKIT_WEB_VIEW(object)));
    emit_text(self, TONTOO_WPE_TITLE, self->title);
}

static void on_notify_url(GObject *object, GParamSpec *spec, gpointer data)
{
    (void)spec;
    TontooWpe *self = data;
    copy_field(self->url, webkit_web_view_get_uri(WEBKIT_WEB_VIEW(object)));
    emit_text(self, TONTOO_WPE_URL, self->url);
}

static void on_notify_progress(GObject *object, GParamSpec *spec, gpointer data)
{
    (void)spec;
    TontooWpe *self = data;
    self->progress = webkit_web_view_get_estimated_load_progress(WEBKIT_WEB_VIEW(object));
    emit_number(self, TONTOO_WPE_PROGRESS, self->progress);
}

static void on_load_changed(WebKitWebView *view, WebKitLoadEvent event, gpointer data)
{
    TontooWpe *self = data;
    switch (event) {
    case WEBKIT_LOAD_STARTED:
        emit_text(self, TONTOO_WPE_LOAD_STARTED, webkit_web_view_get_uri(view));
        break;
    case WEBKIT_LOAD_COMMITTED:
        break;
    case WEBKIT_LOAD_FINISHED:
        emit_text(self, TONTOO_WPE_LOAD_FINISHED, webkit_web_view_get_uri(view));
        break;
    default:
        break;
    }
    /* can_go_back / can_go_forward travel in `number`. */
    TontooWpeEvent *history = event_slot(self);
    if (history) {
        history->kind = TONTOO_WPE_HISTORY;
        history->number = (webkit_web_view_can_go_back(view) ? 1 : 0)
            | (webkit_web_view_can_go_forward(view) ? 2 : 0);
    }
}

static gboolean on_decide_policy(WebKitWebView *view, WebKitPolicyDecision *decision,
                                WebKitPolicyDecisionType type, gpointer data)
{
    (void)view;
    (void)decision;
    (void)type;
    (void)data;
    return TRUE; /* allow */
}

static gboolean on_script_dialog(WebKitWebView *view, gpointer data)
{
    TontooWpe *self = data;
    /*
     * WebKit blocks the page until the dialog is answered. There is no
     * engine-side UI, so accept the default and tell the host, which can
     * show its own dialog through the delegate.
     */
    int64_t id = self->next_dialog_id++;
    emit_number_id(self, TONTOO_WPE_SCRIPT_DIALOG, id, 0 /* ScriptDialogKind::Alert */);
    tontoo_wpe_answer_dialog(self, id, 1, NULL);
    (void)view;
    return TRUE; /* handled */
}

static void on_permission_request(WebKitWebView *view, WebKitPermissionRequest *request,
                                 gpointer data)
{
    TontooWpe *self = data;
    /*
     * The C interface exposes only allow/deny, so the kind stays
     * "other" until the specific policy signals are wired up. Failing
     * closed matches WebViewDelegate::permission_request.
     */
    int64_t id = self->next_dialog_id++;
    emit_number_id(self, TONTOO_WPE_PERMISSION, id, TONTOO_WPE_PERM_OTHER);
    webkit_permission_request_deny(request);
    (void)view;
}

/* ---------------------------------------------------------------------- */
/* JavaScript                                                              */
/* ---------------------------------------------------------------------- */

static void on_javascript_ready(GObject *object, GAsyncResult *result, gpointer data)
{
    int64_t *slot = data;
    int64_t id = *slot;
    GError *error = NULL;
    JSCValue *value = webkit_web_view_call_async_javascript_function_finish(
        WEBKIT_WEB_VIEW(object), result, &error);
    char *owned = NULL;
    if (!value) {
        owned = g_strdup_printf("{\"__error\":\"%s\"}",
                                error ? error->message : "javascript failed");
    } else {
        char *string = jsc_value_to_string(value);
        owned = g_steal_pointer(&string);
        if (!owned)
            owned = g_strdup("null");
    }
    TontooWpe *self = g_object_get_data(G_OBJECT(object), "tontoo-wpe");
    if (self)
        emit_text_id(self, TONTOO_WPE_JS_RESULT, id, owned);
    g_free(owned);
    if (error)
        g_error_free(error);
    g_free(slot);
}

void tontoo_wpe_eval(TontooWpe *self, int64_t id, const char *script)
{
    if (!self || !script)
        return;
    int64_t *slot = malloc(sizeof(int64_t));
    if (!slot)
        return;
    *slot = id;
    /*
     * The wrapper turns any value into JSON so the Rust side can parse it:
     * the C API can only stringify, and `String(obj)` would be useless.
     */
    char *body = g_strdup_printf(
        "return JSON.stringify(await (async () => ( %s ))());", script);
    webkit_web_view_call_async_javascript_function(
        WEBKIT_WEB_VIEW(self->view), body, -1, NULL, NULL, NULL, NULL,
        on_javascript_ready, slot);
    g_free(body);
}

void tontoo_wpe_inject(TontooWpe *self, const char *script)
{
    if (!self || !script)
        return;
    webkit_web_view_call_async_javascript_function(
        WEBKIT_WEB_VIEW(self->view), script, -1, NULL, NULL, NULL, NULL, NULL, NULL);
}

/* ---------------------------------------------------------------------- */
/* cookies                                                                 */
/* ---------------------------------------------------------------------- */

static void append_json_string(GString *out, const char *value)
{
    if (!value) {
        g_string_append(out, "null");
        return;
    }
    g_string_append_c(out, '"');
    for (const char *p = value; *p; p++) {
        switch (*p) {
        case '"': g_string_append(out, "\\\""); break;
        case '\\': g_string_append(out, "\\\\"); break;
        case '\n': g_string_append(out, "\\n"); break;
        case '\r': g_string_append(out, "\\r"); break;
        case '\t': g_string_append(out, "\\t"); break;
        default:
            if ((unsigned char)*p < 0x20)
                g_string_append_printf(out, "\\u%04x", (unsigned char)*p);
            else
                g_string_append_c(out, *p);
        }
    }
    g_string_append_c(out, '"');
}

static void on_cookies_ready(GObject *object, GAsyncResult *result, gpointer data)
{
    int64_t *slot = data;
    int64_t id = *slot;
    GError *error = NULL;
    GList *cookies = webkit_cookie_manager_get_cookies_finish(
        WEBKIT_COOKIE_MANAGER(object), result, &error);
    TontooWpe *self = g_object_get_data(G_OBJECT(object), "tontoo-wpe");
    GString *json = g_string_new("[");
    for (GList *item = cookies; item; item = item->next) {
        SoupCookie *cookie = item->data;
        if (json->len > 1)
            g_string_append_c(json, ',');
        g_string_append(json, "{\"name\":");
        append_json_string(json, soup_cookie_get_name(cookie));
        g_string_append(json, ",\"value\":");
        append_json_string(json, soup_cookie_get_value(cookie));
        g_string_append(json, ",\"domain\":");
        append_json_string(json, soup_cookie_get_domain(cookie));
        g_string_append(json, ",\"path\":");
        append_json_string(json, soup_cookie_get_path(cookie));
        g_string_append_printf(json, ",\"secure\":%s,\"httpOnly\":%s",
                               soup_cookie_get_secure(cookie) ? "true" : "false",
                               soup_cookie_get_http_only(cookie) ? "true" : "false");
        GDateTime *expires = soup_cookie_get_expires(cookie);
        if (expires) {
            g_string_append_printf(json, ",\"expires\":%lld",
                                   (long long)g_date_time_to_unix(expires));
        }
        g_string_append_c(json, '}');
    }
    g_string_append_c(json, ']');
    if (self)
        emit_text_id(self, TONTOO_WPE_COOKIES, id, json->str);
    g_string_free(json, TRUE);
    g_list_free_full(cookies, (GDestroyNotify)g_object_unref);
    if (error)
        g_error_free(error);
    g_free(slot);
}

static WebKitCookieManager *cookie_manager(TontooWpe *self)
{
    WebKitNetworkSession *session = webkit_web_view_get_network_session(self->view);
    return webkit_network_session_get_cookie_manager(session);
}

void tontoo_wpe_list_cookies(TontooWpe *self, int64_t id)
{
    if (!self)
        return;
    int64_t *slot = malloc(sizeof(int64_t));
    if (!slot)
        return;
    *slot = id;
    webkit_cookie_manager_get_cookies(cookie_manager(self), NULL, NULL,
                                      on_cookies_ready, slot);
}

static void on_cookie_added(GObject *object, GAsyncResult *result, gpointer data)
{
    int64_t *slot = data;
    int64_t id = *slot;
    GError *error = NULL;
    webkit_cookie_manager_add_cookie_finish(WEBKIT_COOKIE_MANAGER(object), result, &error);
    TontooWpe *self = g_object_get_data(G_OBJECT(object), "tontoo-wpe");
    if (self)
        emit_number_id(self, TONTOO_WPE_COOKIE_DONE, id, error ? -1 : 0);
    if (error)
        g_error_free(error);
    g_free(slot);
}

static void on_cookie_deleted(GObject *object, GAsyncResult *result, gpointer data)
{
    GError *error = NULL;
    gboolean ok = webkit_cookie_manager_delete_cookie_finish(
        WEBKIT_COOKIE_MANAGER(object), result, &error);
    int64_t *slot = data;
    int64_t id = *slot;
    TontooWpe *self = g_object_get_data(G_OBJECT(object), "tontoo-wpe");
    if (self)
        emit_number_id(self, TONTOO_WPE_COOKIE_DONE, id, ok ? 0 : -1);
    if (error)
        g_error_free(error);
    g_free(slot);
}

void tontoo_wpe_set_cookie(TontooWpe *self, int64_t id, const char *name,
                           const char *value, const char *domain, const char *path,
                           int secure, int http_only, int64_t expires)
{
    if (!self || !name)
        return;
    SoupCookie *cookie = soup_cookie_new(name, value ? value : "",
                                         domain ? domain : "", path ? path : "/",
                                         SOUP_COOKIE_MAX_AGE_ONE_YEAR);
    if (expires > 0) {
        GDateTime *when = g_date_time_new_from_unix_utc((gint64)expires);
        soup_cookie_set_expires(cookie, when);
        g_date_time_unref(when);
    }
    soup_cookie_set_secure(cookie, secure ? TRUE : FALSE);
    soup_cookie_set_http_only(cookie, http_only ? TRUE : FALSE);
    int64_t *slot = malloc(sizeof(int64_t));
    if (!slot) {
        g_object_unref(cookie);
        return;
    }
    *slot = id;
    webkit_cookie_manager_add_cookie(cookie_manager(self), cookie, NULL, on_cookie_added,
                                     slot);
    g_object_unref(cookie);
}

void tontoo_wpe_delete_cookie(TontooWpe *self, int64_t id, const char *name,
                              const char *domain, const char *path)
{
    if (!self || !name)
        return;
    SoupCookie *cookie = soup_cookie_new(name, "", domain ? domain : "",
                                         path ? path : "/",
                                         SOUP_COOKIE_MAX_AGE_ONE_YEAR);
    int64_t *slot = malloc(sizeof(int64_t));
    if (!slot) {
        g_object_unref(cookie);
        return;
    }
    *slot = id;
    webkit_cookie_manager_delete_cookie(cookie_manager(self), cookie, NULL,
                                        on_cookie_deleted, slot);
    g_object_unref(cookie);
}

void tontoo_wpe_clear_data(TontooWpe *self, int cookies, int cache)
{
    if (!self)
        return;
    WebKitNetworkSession *session = webkit_web_view_get_network_session(self->view);
    if (cookies) {
        WebKitWebsiteDataManager *manager =
            webkit_network_session_get_website_data_manager(session);
        if (manager) {
            /* A zero timespan clears everything the type mask selects. */
            webkit_website_data_manager_clear(manager, WEBKIT_WEBSITE_DATA_ALL, 0,
                                              NULL, NULL, NULL);
        }
    }
    if (cache) {
        /* WebKitGTK/WPE keeps the HTTP cache in the website data manager;
         * the cookie wipe above already covers WEBKIT_WEBSITE_DATA_ALL. */
    }
}

/* ---------------------------------------------------------------------- */
/* construction                                                            */
/* ---------------------------------------------------------------------- */

static const struct wpe_view_backend_exportable_fdo_egl_client export_client = {
    .export_egl_image = on_egl_image,
    .export_fdo_egl_image = on_fdo_egl_image,
    .export_shm_buffer = on_shm_buffer,
    ._wpe_reserved0 = NULL,
    ._wpe_reserved1 = NULL,
};

TontooWpe *tontoo_wpe_new(uint32_t width, uint32_t height, const char *url, char *err,
                          size_t err_len)
{
    if (err && err_len)
        err[0] = '\0';
    if (pthread_once(&init_once, process_init) != 0 || init_error[0]) {
        if (err && err_len)
            snprintf(err, err_len, "%s", init_error[0] ? init_error : "wpe init failed");
        return NULL;
    }
    if (width == 0)
        width = 800;
    if (height == 0)
        height = 600;

    TontooWpe *self = calloc(1, sizeof(TontooWpe));
    if (!self) {
        if (err && err_len)
            snprintf(err, err_len, "out of memory");
        return NULL;
    }

    self->exportable = wpe_view_backend_exportable_fdo_egl_create(
        &export_client, self, width, height);
    if (!self->exportable) {
        if (err && err_len)
            snprintf(err, err_len, "wpe_view_backend_exportable_fdo_egl_create failed");
        free(self);
        return NULL;
    }
    self->backend = wpe_view_backend_exportable_fdo_get_view_backend(self->exportable);
    if (!self->backend) {
        if (err && err_len)
            snprintf(err, err_len, "exportable has no view backend");
        goto fail;
    }

    WebKitWebViewBackend *view_backend = webkit_web_view_backend_new(self->backend, NULL, NULL);
    if (!view_backend) {
        if (err && err_len)
            snprintf(err, err_len, "webkit_web_view_backend_new failed");
        goto fail;
    }
    self->view = webkit_web_view_new(view_backend);
    if (!self->view) {
        if (err && err_len)
            snprintf(err, err_len, "webkit_web_view_new failed");
        goto fail;
    }

    g_object_set_data(G_OBJECT(self->view), "tontoo-wpe", self);
    g_object_set_data(G_OBJECT(cookie_manager(self)), "tontoo-wpe", self);

    self->client.set_size = on_set_size;
    self->client.frame_displayed = on_frame_displayed;
    self->client.activity_state_changed = on_activity_state_changed;
    self->client.get_accessible = on_get_accessible;
    self->client.set_device_scale_factor = on_device_scale_factor;
    self->client.target_refresh_rate_changed = on_target_refresh_rate;
    wpe_view_backend_set_backend_client(self->backend, &self->client, self);
    wpe_view_backend_initialize(self->backend);
    wpe_view_backend_dispatch_set_size(self->backend, width, height);

    g_signal_connect(self->view, "notify::title", G_CALLBACK(on_notify_title), self);
    g_signal_connect(self->view, "notify::uri", G_CALLBACK(on_notify_url), self);
    g_signal_connect(self->view, "notify::estimated-load-progress",
                     G_CALLBACK(on_notify_progress), self);
    g_signal_connect(self->view, "load-changed", G_CALLBACK(on_load_changed), self);
    g_signal_connect(self->view, "decide-policy", G_CALLBACK(on_decide_policy), self);
    g_signal_connect(self->view, "script-dialog", G_CALLBACK(on_script_dialog), self);
    g_signal_connect(self->view, "permission-request", G_CALLBACK(on_permission_request),
                     self);

    tontoo_wpe_load(self, url && *url ? url : "about:blank");
    return self;

fail:
    if (self->exportable)
        wpe_view_backend_exportable_fdo_destroy(self->exportable);
    free(self);
    return NULL;
}

void tontoo_wpe_free(TontooWpe *self)
{
    if (!self)
        return;
    /* Let in-flight JavaScript callbacks finish before the view dies. */
    for (int i = 0; i < 50 && self->pending_count > 0; i++) {
        while (g_main_context_pending(NULL))
            g_main_context_iteration(NULL, FALSE);
        g_usleep(2000);
    }
    if (self->view) {
        webkit_web_view_try_close(self->view);
        g_object_unref(self->view);
    }
    if (self->exportable)
        wpe_view_backend_exportable_fdo_destroy(self->exportable);
    free(self->frame);
    free(self);
}

void tontoo_wpe_pump(TontooWpe *self, int ms)
{
    (void)self;
    gint64 deadline = g_get_monotonic_time() + (gint64)ms * 1000;
    do {
        while (g_main_context_pending(NULL))
            g_main_context_iteration(NULL, FALSE);
        g_usleep(1000);
    } while (g_get_monotonic_time() < deadline);
}

/* ---------------------------------------------------------------------- */
/* navigation                                                              */
/* ---------------------------------------------------------------------- */

void tontoo_wpe_load(TontooWpe *self, const char *url)
{
    if (!self || !url || !*url)
        return;
    webkit_web_view_load_uri(self->view, url);
}

void tontoo_wpe_load_html(TontooWpe *self, const char *html, const char *base_uri)
{
    if (!self || !html)
        return;
    webkit_web_view_load_html(self->view, html, base_uri);
}

void tontoo_wpe_reload(TontooWpe *self, int bypass_cache)
{
    if (!self)
        return;
    if (bypass_cache)
        webkit_web_view_reload_bypass_cache(self->view);
    else
        webkit_web_view_reload(self->view);
}

void tontoo_wpe_go(TontooWpe *self, int delta)
{
    if (!self)
        return;
    if (delta < 0)
        webkit_web_view_go_back(self->view);
    else if (delta > 0)
        webkit_web_view_go_forward(self->view);
}

void tontoo_wpe_stop(TontooWpe *self)
{
    if (self)
        webkit_web_view_stop_loading(self->view);
}

void tontoo_wpe_resize(TontooWpe *self, uint32_t width, uint32_t height)
{
    if (!self || width == 0 || height == 0)
        return;
    wpe_view_backend_dispatch_set_size(self->backend, width, height);
}

/* ---------------------------------------------------------------------- */
/* input                                                                   */
/* ---------------------------------------------------------------------- */

void tontoo_wpe_pointer(TontooWpe *self, int kind, double x, double y, uint32_t button)
{
    if (!self)
        return;
    struct wpe_input_pointer_event event = { 0 };
    event.type = kind == TONTOO_WPE_POINTER_MOVE
        ? wpe_input_pointer_event_type_motion
        : wpe_input_pointer_event_type_button;
    event.time = (uint32_t)(g_get_monotonic_time() / 1000);
    event.x = (int)x;
    event.y = (int)y;
    event.button = button;
    /* WPE button state is a bit mask, one bit per button. */
    event.state = (kind == TONTOO_WPE_POINTER_DOWN) ? (1u << button) : 0u;
    event.modifiers = 0;
    wpe_view_backend_dispatch_pointer_event(self->backend, &event);
}

void tontoo_wpe_scroll(TontooWpe *self, double x, double y, double dx, double dy)
{
    if (!self)
        return;
    struct wpe_input_axis_2d_event event = { 0 };
    event.base.type = wpe_input_axis_event_type_motion_smooth;
    event.base.time = (uint32_t)(g_get_monotonic_time() / 1000);
    event.base.x = (int)x;
    event.base.y = (int)y;
    event.base.axis = 0;
    event.base.value = 0;
    event.base.modifiers = 0;
    /* WPE axis deltas are in steps of 15 per notch. */
    event.x_axis = dx / 15.0;
    event.y_axis = dy / 15.0;
    wpe_view_backend_dispatch_axis_event(self->backend, &event.base);
}

void tontoo_wpe_key(TontooWpe *self, uint32_t key_code, int pressed, uint32_t modifiers,
                    const char *text)
{
    if (!self)
        return;
    struct wpe_input_keyboard_event event = { 0 };
    event.time = (uint32_t)(g_get_monotonic_time() / 1000);
    event.key_code = key_code;
    /* No real hardware behind the offscreen view, so the XKB keymap path
     * cannot be used; WebKit falls back to key_code plus modifiers. */
    event.hardware_key_code = key_code;
    event.pressed = pressed ? true : false;
    event.modifiers = modifiers;
    wpe_view_backend_dispatch_keyboard_event(self->backend, &event);
    (void)text;
}

void tontoo_wpe_focus(TontooWpe *self, int focused)
{
    if (!self)
        return;
    if (focused)
        wpe_view_backend_add_activity_state(self->backend, wpe_view_activity_state_focused);
    else
        wpe_view_backend_remove_activity_state(self->backend, wpe_view_activity_state_focused);
}

void tontoo_wpe_answer_dialog(TontooWpe *self, int64_t id, int accept, const char *text)
{
    (void)self;
    (void)id;
    (void)accept;
    (void)text;
}

void tontoo_wpe_answer_permission(TontooWpe *self, int64_t id, int grant)
{
    (void)self;
    (void)id;
    (void)grant;
}

int tontoo_wpe_can_go_back(TontooWpe *self)
{
    return self ? (webkit_web_view_can_go_back(self->view) ? 1 : 0) : 0;
}

int tontoo_wpe_can_go_forward(TontooWpe *self)
{
    return self ? (webkit_web_view_can_go_forward(self->view) ? 1 : 0) : 0;
}

const char *tontoo_wpe_url(TontooWpe *self)
{
    return self ? self->url : "";
}

const char *tontoo_wpe_title(TontooWpe *self)
{
    return self ? self->title : "";
}

double tontoo_wpe_progress(TontooWpe *self)
{
    return self ? self->progress : 0.0;
}
