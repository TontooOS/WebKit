# FFI

The crate is built as a `cdylib` and `rlib`. The C API lives in
`Headers/webkit.h` and lets C / C++ apps and other languages embed a
TontooWebKit web view.

## Loading

On TontooOS the library is installed at `/Library/System/libwebkit.so`
(symlink to `webkit.library`).

```c
#include <webkit.h>

TontooWebView *view = tontoo_webkit_view_new(
    "{\"start_url\": \"https://example.com\", \"private_browsing\": true}",
    NULL);
GtkWidget *widget = tontoo_webkit_view_widget(view);
gtk_window_set_child(GTK_WINDOW(window), widget);
```

## Functions

| Return | Function | Notes |
|---|---|---|
| `const char *` | `tontoo_webkit_version()` | Static version string |
| `TontooWebView *` | `tontoo_webkit_view_new(config_json, error_out)` | `NULL` on error |
| `void` | `tontoo_webkit_view_set_callbacks(view, callbacks, user_data)` | Installs the callback vtable |
| `GtkWidget *` | `tontoo_webkit_view_widget(view)` | Borrowed; do not free |
| `int` | `tontoo_webkit_view_load_url(view, url, error_out)` | `0` on success, `-1` on error |
| `void` | `tontoo_webkit_view_load_html(view, html, base_uri)` | `base_uri` may be `NULL` |
| `void` | `tontoo_webkit_view_go_back(view)` | |
| `void` | `tontoo_webkit_view_go_forward(view)` | |
| `int` | `tontoo_webkit_view_can_go_back(view)` | Nonzero when true |
| `int` | `tontoo_webkit_view_can_go_forward(view)` | Nonzero when true |
| `void` | `tontoo_webkit_view_reload(view)` | |
| `void` | `tontoo_webkit_view_stop_loading(view)` | |
| `char *` | `tontoo_webkit_view_get_url(view)` | Free with `string_free` |
| `char *` | `tontoo_webkit_view_get_title(view)` | Free with `string_free` |
| `int` | `tontoo_webkit_view_is_loading(view)` | Nonzero when true |
| `double` | `tontoo_webkit_view_get_progress(view)` | `0.0..=1.0` |
| `char *` | `tontoo_webkit_view_evaluate_javascript(view, script, error_out)` | JSON result |
| `void` | `tontoo_webkit_string_free(s)` | Frees library strings |
| `void` | `tontoo_webkit_view_free(view)` | Destroys the handle |

## Memory Rules

| Object | Ownership |
|---|---|
| Strings from `get_url`, `get_title`, `evaluate_javascript` | Caller frees with `tontoo_webkit_string_free` |
| `*error_out` strings | Caller frees with `tontoo_webkit_string_free` |
| Widget from `tontoo_webkit_view_widget` | Borrowed; owned by the web view |
| `user_data` | Caller-owned; must outlive installed callbacks |
| `TontooWebView` handle | Freed with `tontoo_webkit_view_free` |

## Configuration JSON

The config string accepts the same shape as `WebKitConfiguration`:

```json
{
  "start_url": "https://example.com",
  "settings": { "javascript_enabled": true, "user_agent": "TontooOS/1.0" },
  "user_scripts": [
    { "source": "window.tontoo = true;", "injection_time": "at_document_start" }
  ],
  "message_handlers": ["ready", "tontoo"],
  "private_browsing": false,
  "data_store": {
    "kind": "default",
    "data_directory": "",
    "cache_directory": ""
  }
}
```

`data_store.kind` is `"default"`, `"ephemeral"` or `"custom"`. Unknown or
missing keys fall back to defaults.

## Callbacks

`TontooWebViewCallbacks` is an optional vtable. Each entry may be `NULL`.
`on_decide_policy` returns nonzero to allow the navigation; the `action`
argument uses the `TONTOO_WEBKIT_POLICY_*` constants from the header.
Script messages arrive as `(name, body_json)`.

## Example

```c
static void on_title(void *user_data, const char *title) {
    g_print("title: %s\n", title ? title : "(none)");
}

TontooWebViewCallbacks cb = {0};
cb.on_title_changed = on_title;
tontoo_webkit_view_set_callbacks(view, cb, NULL);
```

## Cross References

- [WebView.md](WebView.md) -- the underlying Rust API
- [Configuration.md](Configuration.md) -- the JSON shape
- [JavaScript.md](JavaScript.md) -- result mapping
