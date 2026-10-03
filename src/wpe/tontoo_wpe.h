/*
 * tontoo_wpe.h -- WPE WebKit offscreen (OSR) backend for TontooWebKit.
 *
 * One `TontooWpe` owns a headless WebKit view rendered into our own
 * surfaceless EGL display. Rendered frames arrive as wl_shm buffers and are
 * copied into a BGRA buffer the UI uploads as a Vello texture.
 *
 * Threading: every callback (frames, load events, JavaScript results)
 * only writes into a fixed event ring, so no allocation or locking
 * happens on the engine side. The Rust side pumps the GLib main loop and
 * drains the ring.
 */
#ifndef TONTOO_WPE_H
#define TONTOO_WPE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TontooWpe TontooWpe;

/* Event kinds reported through `tontoo_wpe_next_event`. */
enum {
    TONTOO_WPE_NONE = 0,
    TONTOO_WPE_TITLE,          /* text = title */
    TONTOO_WPE_URL,            /* text = url */
    TONTOO_WPE_PROGRESS,       /* number = 0..1e6 (millionths) */
    TONTOO_WPE_LOAD_STARTED,   /* text = url */
    TONTOO_WPE_LOAD_FINISHED,  /* text = url */
    TONTOO_WPE_LOAD_FAILED,    /* text = error */
    TONTOO_WPE_READY,          /* first frame is out */
    TONTOO_WPE_JS_RESULT,      /* id = correlation id, text = JSON */
    TONTOO_WPE_COOKIES,        /* id, text = JSON array */
    TONTOO_WPE_COOKIE_DONE,    /* id */
    TONTOO_WPE_SCRIPT_DIALOG,  /* id, text = message */
    TONTOO_WPE_PERMISSION,     /* id, number = kind */
    TONTOO_WPE_DOWNLOAD,       /* id, text = suggested filename */
    TONTOO_WPE_HISTORY,        /* number = can_back | can_forward<<1 */
    TONTOO_WPE_EXTENSION       /* id, text = extension id */
};

/* Permission kinds reported through TONTOO_WPE_PERMISSION. */
enum {
    TONTOO_WPE_PERM_OTHER = 0,
    TONTOO_WPE_PERM_GEOLOCATION = 1,
    TONTOO_WPE_PERM_NOTIFICATION = 2,
    TONTOO_WPE_PERM_USER_MEDIA = 3,
    TONTOO_WPE_PERM_CAMERA = 4,
    TONTOO_WPE_PERM_MICROPHONE = 5,
    TONTOO_WPE_PERM_MEDIA = 6
};

/* One event record. `text` is always NUL terminated. */
typedef struct {
    int32_t  kind;
    int64_t  id;
    int64_t  number;
    char     text[1024];
} TontooWpeEvent;

/* Pointer button kinds for `tontoo_wpe_pointer`. */
enum {
    TONTOO_WPE_POINTER_MOVE = 0,
    TONTOO_WPE_POINTER_DOWN = 1,
    TONTOO_WPE_POINTER_UP = 2
};

/* Engine version string of the loaded WPE WebKit, or NULL when the
 * library is missing. */
const char *tontoo_wpe_version(void);

/* One line describing the backend, for logs and diagnostics. */
const char *tontoo_wpe_backend_info(void);

/*
 * Create an offscreen engine and load `url` (NULL for about:blank).
 * Returns NULL and fills `err` (a static buffer) on failure.
 */
TontooWpe *tontoo_wpe_new(uint32_t width, uint32_t height, const char *url,
                          char *err, size_t err_len);
void tontoo_wpe_free(TontooWpe *self);

/* Pump the GLib main loop for at most `ms` milliseconds. */
void tontoo_wpe_pump(TontooWpe *self, int ms);

/* Drain one event. Returns 0 when the queue is empty. */
int tontoo_wpe_next_event(TontooWpe *self, TontooWpeEvent *out);

/*
 * Copy the newest rendered frame out as BGRA. Returns 1 when a frame was
 * copied (and bumps `*seq`), 0 when nothing new arrived. The caller owns
 * `*bgra` and frees it with `tontoo_wpe_free_buffer`.
 */
int tontoo_wpe_take_frame(TontooWpe *self, uint8_t **bgra, uint32_t *w,
                          uint32_t *h, uint64_t *seq);
void tontoo_wpe_free_buffer(uint8_t *buffer);

/* Navigation. */
void tontoo_wpe_load(TontooWpe *self, const char *url);
void tontoo_wpe_load_html(TontooWpe *self, const char *html, const char *base_uri);
void tontoo_wpe_reload(TontooWpe *self, int bypass_cache);
void tontoo_wpe_go(TontooWpe *self, int delta);
void tontoo_wpe_stop(TontooWpe *self);
void tontoo_wpe_resize(TontooWpe *self, uint32_t width, uint32_t height);

/* Scripting. `id` is echoed back in TONTOO_WPE_JS_RESULT. */
void tontoo_wpe_eval(TontooWpe *self, int64_t id, const char *script);
void tontoo_wpe_inject(TontooWpe *self, const char *script);

/* Input. Coordinates are CSS pixels. */
void tontoo_wpe_pointer(TontooWpe *self, int kind, double x, double y, uint32_t button);
void tontoo_wpe_scroll(TontooWpe *self, double x, double y, double dx, double dy);
/* `text` may be NULL for non-printable keys. */
void tontoo_wpe_key(TontooWpe *self, uint32_t key_code, int pressed, uint32_t modifiers,
                    const char *text);
void tontoo_wpe_focus(TontooWpe *self, int focused);

/* Cookies. `id` correlates the answers. */
void tontoo_wpe_list_cookies(TontooWpe *self, int64_t id);
void tontoo_wpe_set_cookie(TontooWpe *self, int64_t id, const char *name, const char *value,
                           const char *domain, const char *path, int secure, int http_only,
                           int64_t expires);
void tontoo_wpe_delete_cookie(TontooWpe *self, int64_t id, const char *name,
                              const char *domain, const char *path);
void tontoo_wpe_clear_data(TontooWpe *self, int cookies, int cache);

/* Dialog and permission answers. */
void tontoo_wpe_answer_dialog(TontooWpe *self, int64_t id, int accept, const char *text);
void tontoo_wpe_answer_permission(TontooWpe *self, int64_t id, int grant);

/* Current state. */
int tontoo_wpe_can_go_back(TontooWpe *self);
int tontoo_wpe_can_go_forward(TontooWpe *self);
const char *tontoo_wpe_url(TontooWpe *self);
const char *tontoo_wpe_title(TontooWpe *self);
double tontoo_wpe_progress(TontooWpe *self);

#ifdef __cplusplus
}
#endif

#endif /* TONTOO_WPE_H */
