/*
 * webkit.h -- TontooWebKit C API for TontooOS
 *
 * Web content framework backed by WebKitGTK (Apple WebKit engine). Use
 * tontoo_webkit_view_new() to create a web view, pack
 * tontoo_webkit_view_widget() into a GTK4 container and install callbacks
 * with tontoo_webkit_view_set_callbacks().
 *
 * Strings returned by the library (get_url, get_title,
 * evaluate_javascript, error_out) must be freed with
 * tontoo_webkit_string_free().
 */
#ifndef TONTOO_WEBKIT_H
#define TONTOO_WEBKIT_H

#include <gtk/gtk.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque web view handle. */
typedef struct TontooWebView TontooWebView;

/* Policy action codes passed to on_decide_policy. */
enum {
    TONTOO_WEBKIT_POLICY_NAVIGATION = 0,
    TONTOO_WEBKIT_POLICY_NEW_WINDOW = 1,
    TONTOO_WEBKIT_POLICY_RESPONSE = 2
};

/* Callback vtable. All fields are optional (may be NULL). */
typedef struct {
    void (*on_title_changed)(void *user_data, const char *title);
    void (*on_url_changed)(void *user_data, const char *url);
    void (*on_load_progress)(void *user_data, double progress);
    void (*on_load_started)(void *user_data, const char *url);
    void (*on_load_finished)(void *user_data, const char *url);
    void (*on_load_failed)(void *user_data, const char *url,
                           const char *error);
    void (*on_script_message)(void *user_data, const char *name,
                              const char *body_json);
    void (*on_ready_to_show)(void *user_data);
    int  (*on_decide_policy)(void *user_data, const char *url, int action);
} TontooWebViewCallbacks;

/*
 * Framework version as a static string, e.g. "26.1.0".
 */
const char *tontoo_webkit_version(void);

/*
 * Create a web view from a JSON configuration string.
 *
 * Supported keys:
 *   "start_url"          string  e.g. "https://example.com"
 *   "settings"           object  WebSettings fields (snake_case)
 *   "user_scripts"       array   of { "source", "injection_time",
 *                                    "frames" } objects
 *   "message_handlers"   array   of handler name strings
 *   "private_browsing"   bool
 *   "data_store"         object  { "kind": "default" | "ephemeral" |
 *                                  "custom", "data_directory",
 *                                  "cache_directory" }
 *
 * Returns a new handle or NULL. On failure *error_out receives a message
 * that must be freed with tontoo_webkit_string_free(). Pass NULL as
 * error_out to ignore errors.
 */
TontooWebView *tontoo_webkit_view_new(const char *config_json,
                                      char **error_out);

/*
 * Install the callback vtable and user data. user_data is passed back to
 * every callback and must remain valid while callbacks are installed.
 */
void tontoo_webkit_view_set_callbacks(TontooWebView *view,
                                      TontooWebViewCallbacks callbacks,
                                      void *user_data);

/*
 * The underlying GTK4 widget. The returned pointer is borrowed; add it to
 * a GTK4 container with gtk_box_append() / gtk_window_set_child(). Do not
 * free it.
 */
GtkWidget *tontoo_webkit_view_widget(TontooWebView *view);

/*
 * Load a URL. Returns 0 on success, -1 on error (*error_out set).
 */
int tontoo_webkit_view_load_url(TontooWebView *view, const char *url,
                                char **error_out);

/* Load raw HTML content. base_uri may be NULL. */
void tontoo_webkit_view_load_html(TontooWebView *view, const char *html,
                                  const char *base_uri);

void tontoo_webkit_view_go_back(TontooWebView *view);
void tontoo_webkit_view_go_forward(TontooWebView *view);
int  tontoo_webkit_view_can_go_back(TontooWebView *view);
int  tontoo_webkit_view_can_go_forward(TontooWebView *view);
void tontoo_webkit_view_reload(TontooWebView *view);
void tontoo_webkit_view_stop_loading(TontooWebView *view);

/* Current URL / title, or NULL. Free with tontoo_webkit_string_free(). */
char *tontoo_webkit_view_get_url(TontooWebView *view);
char *tontoo_webkit_view_get_title(TontooWebView *view);

/* Nonzero when the view is loading. */
int tontoo_webkit_view_is_loading(TontooWebView *view);

/* Estimated load progress in [0.0, 1.0]. */
double tontoo_webkit_view_get_progress(TontooWebView *view);

/*
 * Evaluate JavaScript in the page and return the result as a JSON string,
 * or NULL on error (*error_out set). Free with
 * tontoo_webkit_string_free().
 */
char *tontoo_webkit_view_evaluate_javascript(TontooWebView *view,
                                             const char *script,
                                             char **error_out);

/* Free a string returned by this library (incl. error_out strings). */
void tontoo_webkit_string_free(char *s);

/* Destroy a web view handle. The handle must not be used afterwards. */
void tontoo_webkit_view_free(TontooWebView *view);

#ifdef __cplusplus
}
#endif

#endif /* TONTOO_WEBKIT_H */