# FFI

The crate is built as a `cdylib` and `rlib`. One C API exists:
`Headers/webkit_vello.h` — polling model, copy BGRA frames with
`tontoo_vello_view_poll_frame` and upload them as GPU textures. See
[Backend.md](Backend.md) for the frame protocol.

On TontooOS the library is installed at `/Library/System/libwebkit.so`
(symlink to `webkit.library`).

## Configuration JSON

```json
{
  "start_url": "https://example.com",
  "settings": { "javascript_enabled": true, "user_agent": "TontooOS/1.0" },
  "private_browsing": false,
  "spawn_engine": true
}
```

Unknown or missing keys fall back to defaults.

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
