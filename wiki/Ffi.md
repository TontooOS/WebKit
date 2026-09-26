# FFI

The crate is built as a `cdylib` and `rlib`. Two C APIs exist:

- `Headers/webkit.h` (this page): GTK backend (`gtk-backend` feature),
  widget embedding with a callback vtable.
- `Headers/webkit_vello.h`: Vello backend (`vello` feature, no GTK),
  polling model -- copy BGRA frames with
  `tontoo_vello_view_poll_frame` and upload them as GPU textures. See
  [Backend.md](Backend.md) for the frame protocol.

## Loading (GTK)

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

## Vello Functions (`webkit_vello.h`)

| Return | Function | Notes |
|---|---|---|
| `TontooVelloView *` | `tontoo_vello_view_new(config_json, error_out)` | `NULL` on error; `"spawn_engine": true` uses the helper |
| `int` | `tontoo_vello_view_pump(view)` | Apply queued engine events; call once per UI frame |
| `int` | `tontoo_vello_view_frame_size(view, w, h)` | `1` when a frame exists |
| `int` | `tontoo_vello_view_poll_frame(view, dst, dst_len, w, h, error_out)` | `1` copied, `0` unchanged, `-1` on error; BGRA bytes |
| `int` | `tontoo_vello_view_load_url(view, url, error_out)` | `0` on success, `-1` on error |
| `void` | `tontoo_vello_view_load_html(view, html, base_uri)` | `base_uri` may be `NULL` |
| `void` | `tontoo_vello_view_resize(view, width, height, scale)` | Physical pixels plus UI scale |
| `void` | `tontoo_vello_view_mouse_down/up/move(view, x, y)` | Logical px |
| `void` | `tontoo_vello_view_scroll(view, dx, dy)` | Logical px |
| `void` | `tontoo_vello_view_key_text(view, text)` | UTF-8, NUL-terminated |
| `void` | `tontoo_vello_view_key_press(view, key)` | `Enter`, `Backspace`, `Escape`, arrows |
| `char *` | `tontoo_vello_view_evaluate_javascript(view, script, error_out)` | JSON string, blocks up to 5 s, free with `string_free` |
| `int` | `tontoo_vello_view_poll_script_message(view, name_out, body_json_out)` | `1` message, `0` empty, `-1` on NULL handle |
| `char *` | `tontoo_vello_view_get_url(view)` / `tontoo_vello_view_get_title(view)` | Free with `string_free` |
| `int` / `double` | `tontoo_vello_view_is_loading(view)` / `tontoo_vello_view_get_progress(view)` | Progress in [0.0, 1.0] |

```c
#include <webkit_vello.h>

TontooVelloView *view = tontoo_vello_view_new("{\"spawn_engine\": true}", NULL);
uint32_t w = 0, h = 0;
tontoo_vello_view_resize(view, 800, 600, 1.0f);
tontoo_vello_view_load_url(view, "https://example.com", NULL);
tontoo_vello_view_pump(view);
if (tontoo_vello_view_frame_size(view, &w, &h)) {
  uint8_t *pixels = malloc((size_t)w * h * 4);
  if (tontoo_vello_view_poll_frame(view, pixels, (size_t)w * h * 4, NULL, NULL, NULL) == 1) {
    upload_as_gpu_texture(pixels, w, h);
  }
  free(pixels);
}
```

## Cross References

- [WebView.md](WebView.md) -- the underlying Rust API
- [Configuration.md](Configuration.md) -- the JSON shape
- [JavaScript.md](JavaScript.md) -- result mapping
- [Backend.md](Backend.md) -- engine trait, helper protocol, WPE plan
