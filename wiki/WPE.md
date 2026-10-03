# WPE Backend

The default engine. Real WebKit, rendered offscreen inside the TontooUI
window: no engine window, no compositor round trip, one surface.

## How It Works

```
WpeEngine (src/wpe.rs)          worker thread "tontoo-wpe"
  |                                |
  | tontoo_wpe_new()               |  wpe_loader_init("libWPEBackend-fdo-1.0.so")
  |                                |  wpe_fdo_initialize_for_egl_display(surfaceless EGL)
  |                                |  wpe_view_backend_exportable_fdo_egl_create()
  |                                |  webkit_web_view_backend_new()
  |                                |  webkit_web_view_new()
  | <-- SharedFrame (BGRA) <-------+  export_shm_buffer -> wl_shm_buffer copy
  | <-- EngineEvent     <---------+  event ring (title, url, progress, load,
  |                                     JS results, cookies, dialogs, history)
```

1. The build script resolves the engine through `pkg-config` and compiles
   `src/wpe/tontoo_wpe.c`; the shim owns EGL, libwpe and WebKit.
2. `WpeEngine::launch` spawns one worker thread and creates the view there.
   The UI thread only sends `EngineCommand`s and reads the newest frame, so
   no GLib, EGL or WebKit call ever happens on the UI thread.
3. The worker pumps the GLib main loop in 8 ms slices, drains the shim's event
   ring into `EngineEvent`s and copies the newest finished frame.
4. `WebViewContent` uploads that `SharedFrame` as a Vello texture, so the
   page is just another TontooUI view: rounded corners, LiquidGlass and the
   toolbar all work unchanged.

The offscreen route uses the FDO backend's *exportable* view backend: the
shim creates a surfaceless EGL display (`EGL_PLATFORM_SURFACELESS_MESA`),
asks the FDO backend for an exportable EGL view backend and hands it to
`webkit_web_view_backend_new`. WebKit then paints into
`wl_shm_buffer`s, which the shim copies into a BGRA buffer. No Wayland
display connection is needed, so the engine also works headless (CI).

## Build And Run Dependencies

| Package | Provides |
|---|---|
| `wpewebkit` | `wpe-webkit-2.0.pc`, the WebKit C API |
| `libwpe` | `wpe-1.0.pc`, the embedder API |
| `wpebackend-fdo` | `wpebackend-fdo-1.0.pc`, the offscreen exportable |
| `glib2` | `glib-2.0.pc`, main loop and signals |
| `mesa` / `libegl` | `egl.pc`, surfaceless EGL display |
| `wayland` | `wayland-server.pc`, `wl_shm_buffer` accessors |

Arch names:

```sh
pacman -S --needed wpewebkit libwpe wpebackend-fdo
```

`build.rs` only emits `cargo:warning=tontoo-wpe: ...` and the
`tontoo_wpe_missing` cfg when they are missing, so the crate still compiles
without the engine; `WebViewContent::with_wpe` then fails and callers fall
back to the placeholder engine.

> Note: `libWPEWebKit-2.0.so.1` from Arch requires a newer glibc than some
> older distributions ship. `ldd --version` inside the build environment has
> to satisfy the `GLIBC_2.44` symbols, otherwise the loader refuses the
> library at run time.

## Using It

```rust,no_run
use webkit::{WebKitConfiguration, WebViewContent};

// TontooUI window: the page is drawn inside it.
let web = WebViewContent::with_wpe(
    WebKitConfiguration::new().start_url("https://example.com"),
)?;

// Or drive the engine directly.
let engine = webkit::WpeEngine::launch(webkit::WpeOptions {
    width: 1200,
    height: 800,
    start_url: Some("https://example.com".into()),
    private: false,
    download_dir: std::env::temp_dir().join("tontoo-webengine"),
})?;
let view = webkit::WebView::with_engine(
    WebKitConfiguration::new(),
    std::sync::Arc::new(engine),
)?;
```

`cargo run --example tontoo_browser` starts the full browser: toolbar,
address field, progress and status line in one window.

| Env | Meaning |
|---|---|
| `TONTOO_BROWSER_HOME` | Start URL (default `https://example.com`) |
| `TONTOO_BROWSER_PRIVATE` | `1` starts with an ephemeral data store |
| `TONTOO_BROWSER_MOCK` | `1` uses the placeholder engine instead of WPE |

## What The Shim Provides

`src/wpe/tontoo_wpe.h` is the C surface; `src/wpe.rs` mirrors it 1:1.

| Area | Functions |
|---|---|
| Lifecycle | `tontoo_wpe_new`, `tontoo_wpe_free`, `tontoo_wpe_pump`, `tontoo_wpe_focus` |
| Frames | `tontoo_wpe_take_frame`, `tontoo_wpe_free_buffer` |
| Events | `tontoo_wpe_next_event` (ring of title, url, progress, load, JS, cookies, dialog, permission, history) |
| Navigation | `tontoo_wpe_load`, `tontoo_wpe_load_html`, `tontoo_wpe_reload`, `tontoo_wpe_go`, `tontoo_wpe_stop`, `tontoo_wpe_resize`, `tontoo_wpe_can_go_back`, `tontoo_wpe_can_go_forward`, `tontoo_wpe_url`, `tontoo_wpe_title`, `tontoo_wpe_progress` |
| Input | `tontoo_wpe_pointer`, `tontoo_wpe_scroll`, `tontoo_wpe_key` |
| Script | `tontoo_wpe_eval`, `tontoo_wpe_inject` |
| Cookies | `tontoo_wpe_list_cookies`, `tontoo_wpe_set_cookie`, `tontoo_wpe_delete_cookie`, `tontoo_wpe_clear_data` |
| Dialogs | `tontoo_wpe_answer_dialog`, `tontoo_wpe_answer_permission` |
| Introspection | `tontoo_wpe_version`, `tontoo_wpe_backend_info` |

Implementation notes:

- Frames arrive as `wl_shm_buffer` in `ARGB8888`; the shim converts them to
  BGRA, which is what `SharedFrame` and the Vello texture path expect.
- Only the newest frame is kept (`seq` counter), so a slow UI drops frames
  instead of queueing them.
- JavaScript answers are asynchronous: `eval_sync` returns `None` and the
  result arrives as an `EngineEvent::JsResult` with the request id.
- Cookies are libsoup 3 `SoupCookie`s (`webkit_cookie_manager_add_cookie`).
- Permission requests are denied by default, matching the crate's
  delegate policy; the pending request is answered through
  `WebViewDelegate::permission_request`.

## Tests

`tests/wpe_osr.rs` is the end-to-end proof: it launches the engine
headless, loads a page with a green background, waits for
`EngineEvent::LoadFinished` plus a painted frame and asserts the frame is
640x480 BGRA with green pixels.

```sh
cargo test --test wpe_osr
```

Mesa prints `ZINK: failed to choose pdev` warnings in a headless WSL
session; they are harmless because the surfaceless software path takes over.

## Limitations

- One view per process is the tested path. A second `WpeEngine` in the same
  process reuses the loader and EGL display, which libwpe only initializes
  once.
- Document-start injection (`ScriptFrameInjection`) is exposed in the shim
  (`tontoo_wpe_inject`) but not wired to a Rust command yet.
- No WebExtensions: WebKitGTK/WPE has no add-on manager. Use the `gecko`
  feature for that (see [Extensions.md](Extensions.md)).