# Backend

How TontooWebKit renders HTML, CSS and JavaScript without depending on a
single engine or toolkit. The public API (`WebView`, `WebKitConfiguration`,
delegates) stays stable while the engine behind it is exchangeable.

## Design

```
WebView (src/view.rs, no toolkit dependency)
  |-- WebEngine trait (src/engine.rs)
  |     |-- GeckoEngine ..... headful Firefox over WebDriver BiDi
  |     |     (in-process: two worker threads, no helper needed)
  |     |-- ProcessEngine ... spawns tontoo-webengine (src/transport.rs)
  |     |     |-- GeckoRenderer  .. same GeckoPage, line protocol
  |     |     +-- MockRenderer   .. animated placeholder + mock cookies
  |     +-- MockEngine ........ checkerboard, unit tests
  |-- SharedFrame { seq, width, height, rgba }   (pixel-producing engines)
  +-- EngineCommand / EngineEvent (JSON IPC vocabulary, manual encoding)
```

Two ways to reach Gecko:

| Entry point | Use it when | Cost |
|---|---|---|
| [`gecko::GeckoEngine`] | Browser apps; talk to Firefox directly | No helper binary, no line protocol |
| [`transport::ProcessEngine`] | The engine must be isolated, or reached through the C ABI | One extra process, line protocol |

> **Note:** Gecko draws into its **own Wayland window**, so it never
> produces a [`SharedFrame`]. `WebEngine::latest_frame` returns `None` and
> the engine protocol reports `EngineEvent::EngineWindow { pid, kiosk }`
> instead. A host app looks that window up through
> `CoreWindows::list_windows` (match on `pid`) to position, minimize or
> close it. Frame-producing engines (a future WPE WebKit helper, CEF OSR)
> keep the `SharedFrame` path and the TontooUI texture blit.

The view owns no widget. It sends [`EngineCommand`] values to the engine
and reads the latest [`SharedFrame`] with `poll_frame` when the engine has
one. TontooUI blits that frame as a Vello texture (see
`tontooui_view::WebViewContent`), so rounded corners and LiquidGlass keep
working. Input flows the other way: mouse, wheel and typed text become
engine commands. Queued engine events are applied with `pump_events`,
which every UI calls once per frame.

## Cargo Features

| Feature | Default | Description |
|---|---|---|
| `vello` | yes | Backend-neutral `WebView`, `WebViewContent` for TontooUI |
| `gecko` | yes | Headful Firefox over WebDriver BiDi (pages, extensions) |

`WebView` is the engine view in all configurations.

## API

### `WebEngine`

```rust
pub trait WebEngine: Send + Sync {
  fn send(&self, command: EngineCommand);
  fn latest_frame(&self) -> Option<Arc<SharedFrame>>;
  fn load_url(&self, url: &str) -> Result<(), WebKitError>;
  fn load_html(&self, html: &str, base_uri: Option<&str>);
  fn resize(&self, width: u32, height: u32, scale: f32);
  fn drain_events(&self) -> Vec<EngineEvent> { Vec::new() }
  fn eval_sync(&self, script: &str) -> Option<JsonValue> { None }
}
```

- Every method only enqueues a command and returns immediately; the
  engine runs on its own thread or process.
- `load_url` validates the scheme (`http`, `https`, `file`, `data`,
  `about`) and returns `Err(WebKitError::InvalidUrl)` otherwise.
- `eval_sync` answers inline when the engine lives in this process
  (`GeckoEngine`, `MockEngine`); out-of-process engines return `None` and
  the view correlates the async `JsResult` event instead.

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

JSON enums forming the IPC vocabulary between the UI process and the
engine process (`to_json_string` / `from_json_str` in `engine.rs`, same
shape as the former serde encoding). Commands cover navigation
(`LoadUrl`, `LoadHtml`,
`Reload`, `GoBack`, `GoForward`, `Stop`), scripting (`EvaluateJs`),
sizing (`Resize`) and input (`MouseDown`, `MouseUp`, `MouseMove`,
`Scroll`, `KeyText`). Events report frames (`FrameReady` carries seq and
size only; pixels travel via shared memory), state (`Title`, `Url`,
`Progress`, `LoadStarted`, `LoadFinished`, `LoadFailed`,
`ScriptMessage`, `JsResult`) and readiness (`ReadyToShow`).

Gecko adds the browser-shaped parts:

| Command | Event |
|---|---|
| `InstallExtension { id, path }` | `ExtensionInstalled { id, extension }` / `ExtensionFailed { id, error }` |
| `ListExtensions { id }` | `Extensions { id, extensions }` |
| `NewTab { id, kind }` | `ContextReady { context }` |
| `CloseTab { id, context }` | `ContextClosed { context }` |
| `ActivateTab { id, context }` | -- |
| -- | `EngineWindow { pid, kiosk }` |

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
  exit, for headless self tests), `--headless` (no Firefox window),
  `--private` (ephemeral profile), `--renderer=mock|gecko|auto`.
- stdin: `C {<EngineCommand JSON>}` lines. stdout: `E {<EngineEvent
  JSON>}` lines. stderr is inherited for logs.
- Frames: raw BGRA bytes in `<slot-dir>/frame.bgra`, announced by
  `FrameReady { seq, width, height }`. The UI re-reads the file only on a
  new event, so pixels never cross the pipes. Gecko writes no frames.
- Renderer: `gecko` (feature `gecko`, Firefox binary present) drives a
  real Firefox; `mock` is the animated software placeholder with a mock
  cookie jar and the fallback when no Firefox exists.
- Headless self test: `tontoo-webengine --slot-dir <dir> --ping --renderer=mock`
  prints `FrameReady` and writes exactly `800 * 600 * 4` bytes.

## Gecko Renderer (Production)

The Gecko driver in [`Gecko.md`](Gecko.md) drives a headful Firefox over
WebDriver BiDi. Page JavaScript, WebGL, video, printing and
WebExtensions all run inside Gecko; host processes only ever see JSON.

## Firefox Auto-Update (`gecko_provision.rs`)

The helper provisions and updates its own Firefox ESR build, so
TontooOS installations run a patched Gecko even without a distro package
(see [Gecko.md](Gecko.md) for the full layout and env table).

End-to-end proof (helper binary plus a Firefox binary required):

```bash
TONTOO_WEBENGINE_BIN=tontoo-webengine TONTOO_WEBENGINE_RENDERER=mock cargo test --test process
cargo test --test gecko_bidi -- --ignored --nocapture
```

The first test round-trips the line protocol against the mock renderer.
The second launches a headless private Firefox, asserts `1 + 1 == 2` and
`document.title == "Hello"` through the full stack, round-trips the
cookie jar and lists the installed extensions. It is `#[ignore]`d
because a real browser session takes minutes to set up on a
software-rendered host; `TONTOO_E2E_TIMEOUT_SECS` raises the per-step
budget.

## Usage / Example

```rust,no_run
use std::sync::Arc;
use webkit::{GeckoEngine, GeckoOptions, MockEngine, WebKitConfiguration, WebView};

// Real browser: Firefox owns the window, TontooUI draws the toolbar.
let engine = GeckoEngine::launch(GeckoOptions::default()).expect("Firefox");
let view = WebView::builder()
  .start_url("https://example.com")
  .build_on(Arc::new(engine))
  .expect("invalid start URL");

// Offline host: the test engine.
let engine = Arc::new(MockEngine::new(800, 600));
let view = WebView::builder()
  .start_url("https://example.com")
  .build_on(engine)
  .expect("invalid start URL");
view.resize(800, 600, 1.0);
let frame = view.poll_frame().expect("engine serves frames");
assert_eq!(frame.rgba.len(), 800 * 600 * 4);
```

Run the demos:

```bash
cargo run --example tontoo_browser   # TontooUI toolbar + headful Firefox
cargo run --example vello_webview    # mock frames in a TontooUI window
cargo run --example vello_browser    # mock frames with a browser toolbar
```

## Cross References

- [Gecko.md](Gecko.md) -- the Firefox driver, profile prefs, BiDi mapping
- [Extensions.md](Extensions.md) -- WebExtensions install, list, profiles
- [WebView.md](WebView.md) -- the backend-neutral view widget and its methods
- [Ffi.md](Ffi.md) -- C API (`webkit_vello.h`)
- [Settings.md](Settings.md) -- engine settings
