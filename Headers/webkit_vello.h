/*
 * webkit_vello.h -- TontooWebKit C API for Vello/WGPU hosts (TontooUI)
 *
 * Polling model without any GTK dependency: drive the view, copy BGRA
 * frames with tontoo_vello_view_poll_frame() and upload them as GPU
 * textures. Frames are width * height * 4 bytes, BGRA, top-down.
 *
 * Strings returned by the library (get_url, get_title,
 * evaluate_javascript, poll_script_message, error_out) must be freed with
 * tontoo_vello_string_free(). Frame buffers are owned by the caller.
 */
#ifndef TONTOO_WEBKIT_VELLO_H
#define TONTOO_WEBKIT_VELLO_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque web view handle. */
typedef struct TontooVelloView TontooVelloView;

/*
 * Framework version as a static string, e.g. "26.1.0".
 */
const char *tontoo_vello_version(void);

/*
 * Create a web view from a JSON configuration string.
 *
 * Supported keys:
 *   "start_url"        string  e.g. "https://example.com"
 *   "settings"         object  WebSettings fields (snake_case)
 *   "private_browsing" bool
 *   "spawn_engine"     bool    spawn tontoo-webengine when available
 *
 * Returns a new handle or NULL. On failure *error_out receives a message
 * that must be freed with tontoo_vello_string_free(). Pass NULL as
 * error_out to ignore errors.
 */
TontooVelloView *tontoo_vello_view_new(const char *config_json,
                                       char **error_out);

/* Destroy a web view handle. The handle must not be used afterwards. */
void tontoo_vello_view_free(TontooVelloView *view);

/*
 * Apply queued engine events. Call once per UI frame before reading
 * state or polling frames. Returns the number of applied events.
 */
int tontoo_vello_view_pump(TontooVelloView *view);

/*
 * Latest frame size in physical pixels. Returns 1 when a frame exists,
 * 0 otherwise. w/h may be NULL.
 */
int tontoo_vello_view_frame_size(TontooVelloView *view, uint32_t *w,
                                 uint32_t *h);

/*
 * Copy the latest frame as BGRA bytes into dst (dst_len bytes).
 *
 * Returns 1 when a new frame was copied, 0 when the frame is unchanged,
 * -1 on error (*error_out set). w/h receive the frame size when non-NULL.
 */
int tontoo_vello_view_poll_frame(TontooVelloView *view, void *dst,
                                 size_t dst_len, uint32_t *w, uint32_t *h,
                                 char **error_out);

/*
 * Load a URL. Returns 0 on success, -1 on error (*error_out set).
 */
int tontoo_vello_view_load_url(TontooVelloView *view, const char *url,
                               char **error_out);

/* Load raw HTML content. base_uri may be NULL. */
void tontoo_vello_view_load_html(TontooVelloView *view, const char *html,
                                 const char *base_uri);

/* Allocated size in physical pixels plus UI scale factor. */
void tontoo_vello_view_resize(TontooVelloView *view, uint32_t width,
                              uint32_t height, float scale);

/* Input in view coordinates (logical px). */
void tontoo_vello_view_mouse_down(TontooVelloView *view, double x, double y);
void tontoo_vello_view_mouse_up(TontooVelloView *view, double x, double y);
void tontoo_vello_view_mouse_move(TontooVelloView *view, double x, double y);
void tontoo_vello_view_scroll(TontooVelloView *view, double dx, double dy);
void tontoo_vello_view_key_text(TontooVelloView *view, const char *text);

void tontoo_vello_view_go_back(TontooVelloView *view);
void tontoo_vello_view_go_forward(TontooVelloView *view);
void tontoo_vello_view_reload(TontooVelloView *view);
void tontoo_vello_view_stop_loading(TontooVelloView *view);

/*
 * Evaluate JavaScript, blocking up to 5 seconds for the JSON result.
 * Returns NULL on error (*error_out set). Free with
 * tontoo_vello_string_free().
 */
char *tontoo_vello_view_evaluate_javascript(TontooVelloView *view,
                                            const char *script,
                                            char **error_out);

/*
 * Take one queued script message. Returns 1 when a message was written
 * to *name_out/*body_json_out (both must be freed), 0 when the queue is
 * empty, -1 on a NULL handle.
 */
int tontoo_vello_view_poll_script_message(TontooVelloView *view,
                                           char **name_out,
                                           char **body_json_out);

/* Current URL / title, or NULL. Free with tontoo_vello_string_free(). */
char *tontoo_vello_view_get_url(TontooVelloView *view);
char *tontoo_vello_view_get_title(TontooVelloView *view);

/* Nonzero when the view is loading. */
int tontoo_vello_view_is_loading(TontooVelloView *view);

/* Estimated load progress in [0.0, 1.0]. */
double tontoo_vello_view_get_progress(TontooVelloView *view);

/* Free a string returned by this library (incl. error_out strings). */
void tontoo_vello_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* TONTOO_WEBKIT_VELLO_H */
