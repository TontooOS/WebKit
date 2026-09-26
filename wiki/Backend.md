# Backend

How TontooWebKit renders HTML, CSS and JavaScript without depending on a
single engine or toolkit. The public API (`WebView`, `WebKitConfiguration`,
delegates) stays stable while the engine behind it is exchangeable.

## Design

```
WebView (src/view.rs, no toolkit dependency)
  |-- WebEngine trait (src/engine.rs)
  |     |-- MockEngine ......... checkerboard, unit tests
  |     |-- ProcessEngine ...... spawns tontoo-webengine (src/transport.rs)
  |     +-- WpeEngine (planned)  WPE WebKit inside the helper
  |-- SharedFrame { seq, width, height, rgba }
  +-- EngineCommand / EngineEvent (serializable IPC vocabulary)
```

The view owns no widget. It sends [`EngineCommand`] values to the engine
and reads the latest [`SharedFrame`] with `poll_frame`. TontooUI blits the
frame as a Vello texture (see `tontooui_view::WebViewContent`), so rounded
corners and LiquidGlass keep working. Input flows the other way: mouse,
wheel and typed text become engine commands. Queued engine events are
applied with `pump_events`, which every UI calls once per frame.

## Cargo Features

| Feature | Default | Description |
|---|---|---|
| `vello` | yes | Backend-neutral `WebView`, `WebViewContent` for TontooUI |
| `gtk-backend` | no | Legacy WebKitGTK `GtkWebView` plus C FFI and cookie/data-store managers |

With both features enabled the legacy names keep working (`WebView` is
the GTK view) and the new view is exported as `VelloWebView`,
`VelloWebViewBuilder` and `VelloWebViewContent`. Without `gtk-backend`
`WebView` is the new engine view.

## API

### `WebEngine`

```rust
pub trait WebEngine: Send + Sync {
  fn send(&self, command: EngineCommand);
  fn latest_frame(&self) -> Option<Arc<SharedFrame>>;
  fn load_url(&self, url: &str) -> Result<(), WebKitError>;
  fn load_html(&self, html: &str, base_uri: Option<&str>);
  fn resize(&self, width: u32, height: u32, scale: f32);
}
```

- Every method only enqueues a command and returns immediately; the
  engine runs on its own thread or process.
- `load_url` validates the scheme (`http`, `https`, `file`, `data`,
  `about`) and returns `Err(WebKitError::InvalidUrl)` otherwise.

### `SharedFrame`

```rust
pub struct SharedFrame {
  pub seq: u64,
  pub width: u32,
  pub height: u32,
  pub rgba: Vec<u8>,
}
```

- `rgba` holds exactly `width * height * 4` BGRA bytes.
- `seq` is a monotonic counter; the UI reuses its GPU upload while it
  matches (`ImageLoader::upload_frame` in TontooUI).
- `SharedFrame::checkerboard(seq, width, height)` renders the dark-mode
  placeholder used before the first real frame.

### `MockEngine`

```rust
pub struct MockEngine { /* ... */ }
```

- Serves a checkerboard frame, records the last URL and answers every
  JavaScript evaluation with `null` inline (`eval_sync`).
- `resize_frame(width, height)` re-renders the placeholder at a new size.
- `last_url() -> Option<String>` returns the last loaded URL.
- Used by unit tests; the helper binary is the integration stand-in.

### `ProcessEngine`

```rust
pub struct ProcessEngine { /* ... */ }
pub fn find_helper() -> Result<PathBuf, WebKitError>
```

- Spawns the `tontoo-webengine` helper (see below) and implements
  `WebEngine` over pipes plus a frame file.
- Lookup order: `TONTOO_WEBENGINE_BIN`, then walking up from the current
  executable (covers cargo `examples/`, `deps/` and plain `debug/` or
  `release/` layouts), then `PATH`.
- `cargo run --example` does not build the helper, so run `cargo build`
  (or `cargo build --bin tontoo-webengine`) once first. Without a helper
  binary `WebView::with_spawned_engine(config)` falls back to
  `MockEngine` with a stderr note, so apps and demos still start.

### `EngineCommand` / `EngineEvent`

Serializable enums forming the IPC vocabulary between the UI process and
the engine process. Commands cover navigation (`LoadUrl`, `LoadHtml`,
`Reload`, `GoBack`, `GoForward`, `Stop`), scripting (`EvaluateJs`),
sizing (`Resize`) and input (`MouseDown`, `MouseUp`, `MouseMove`,
`Scroll`, `KeyText`). Events report frames (`FrameReady` carries seq and
size only; pixels travel via shared memory), state (`Title`, `Url`,
`Progress`, `LoadStarted`, `LoadFinished`, `LoadFailed`,
`ScriptMessage`, `JsResult`) and readiness (`ReadyToShow`).

`WebView::apply_event` feeds one event into the delegates; the transport
reader thread queues them and `WebView::pump_events` applies them on the
UI thread. Dialog answers come from `ScriptDialogRef::set_confirmed` /
`set_prompt_text` (recorded, then sent as `DialogAnswer`); permission
answers come from the `WebViewDelegate::permission_request` return value;
download destinations come from `DownloadDelegate::decide_destination`.
JavaScript and cookie listings correlate by id (`JsResult`, `Cookies`
events); `evaluate_javascript` and `list_cookies` block up to 5 seconds
(`JS_TIMEOUT`) while pumping.

## Helper Protocol (`tontoo-webengine`)

The helper (`src/bin/tontoo-webengine.rs`) is a plain child process:

- Args: `--slot-dir <dir>` (required), `--ping` (render one frame and
  exit, for headless self tests).
- stdin: `C {<EngineCommand JSON>}` lines. stdout: `E {<EngineEvent
  JSON>}` lines. stderr is inherited for logs.
- Frames: raw BGRA bytes in `<slot-dir>/frame.bgra`, announced by
  `FrameReady { seq, width, height }`. The UI re-reads the file only on a
  new event, so pixels never cross the pipes.
- The current renderer is an animated software placeholder (band moves on
  input/resize; top strip echoes the URL length) with a mock cookie jar.
- Headless self test: `tontoo-webengine --slot-dir <dir> --ping` prints
  `FrameReady` and writes exactly `800 * 600 * 4` bytes.

## WPE Slot (Planned)

The production renderer is WPE WebKit (same Apple WebKit engine as the
legacy GTK backend, but Wayland-native without GTK) inside the helper.
Swap `render()` for the `wpe_view_backend_exportable_fdo` readback and
everything upstream (view, TontooUI embedding, C ABI) keeps working
unchanged:

- Renders offscreen through `wpe_view_backend_exportable_fdo` (EGL) and
  exports BGRA frames via `memfd` shared memory plus damage rects.
- Runs sandboxed (`bubblewrap`, `xdg-dbus-proxy`); a page crash never
  kills the app process.
- System packages on TontooOS: `wpewebkit`, `libwpe`, `wpebackend-fdo`.

## Usage / Example

```rust,no_run
use std::sync::Arc;
use webkit::{MockEngine, WebKitConfiguration, WebView};

let engine = Arc::new(MockEngine::new(800, 600));
let web_view = WebView::builder()
  .start_url("https://example.com")
  .build_on(engine)
  .expect("invalid start URL");

web_view.resize(800, 600, 1.0);
let frame = web_view.poll_frame().expect("engine serves frames");
assert_eq!(frame.rgba.len(), 800 * 600 * 4);
```

Run the TontooUI demos (helper frames in a real window; falls back to
the test engine when the helper is missing):

```bash
cargo run --example vello_webview
cargo run --example vello_browser
```

Headless round-trip test (needs the built helper binary):

```bash
TONTOO_WEBENGINE_BIN=../../target/debug/tontoo-webengine cargo test --test process
```

## Cross References

- [WebView.md](WebView.md) – the backend-neutral view widget and its methods
- [UIKit.md](UIKit.md) – legacy GTK embedding (deprecated shim)
- [Ffi.md](Ffi.md) – C APIs (`webkit.h` for GTK, `webkit_vello.h` for Vello)
- [Settings.md](Settings.md) – settings are serializable for the engine helper
