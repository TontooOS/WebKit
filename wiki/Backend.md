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
  |     |     |-- MockRenderer . animated placeholder + mock cookies
  |     |     +-- ChromiumRenderer (CDP, real pages in the sandbox)
  |     +-- CefOsr (planned) ... CEF offscreen rendering, same protocol
  |-- SharedFrame { seq, width, height, rgba }
  +-- EngineCommand / EngineEvent (JSON IPC vocabulary, manual encoding)
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
| `chromium` | yes | Headless Chromium over CDP (real pages, screenshots, JS) |

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

## Chromium Renderer (Production)

The helper drives headless Chromium over the DevTools protocol
(`src/chromium.rs`, cargo feature `chromium`, on by default). Page
JavaScript runs inside the real Chromium sandbox with site isolation;
our processes only ever see screenshots, JSON values and cookies.

- Launch: `chromium --headless=new --remote-debugging-port=<free>`,
  sandbox left ON, background networking and component updates off.
  Flags are fixed in `chrome_args`; the sandbox is only disabled with
  `TONTOO_CHROME_NO_SANDBOX=1` (containers without user namespaces,
  never for daily use).
- Binary lookup: `CHROMIUM_BIN`/`CHROME_BIN`, the managed
  self-provisioned build, then `chromium`, `chromium-browser`,
  `google-chrome`, `google-chrome-stable`, `chrome` on `PATH`
  (smoke-tested, broken builds are skipped). Without any binary the
  helper falls back to mock.
- One page target plus the browser endpoint (two CDP sessions):
  navigation, history (`Page.getNavigationHistory`), screenshots
  (`Page.captureScreenshot` decoded to BGRA), input (`Input.*`),
  JavaScript (`Runtime.evaluate` with real values), cookies
  (`Storage.*`), downloads (`Page.setDownloadBehavior` allow into the
  slot dir, `Browser.downloadWillBegin/Progress`, `Browser.cancelDownload`)
  and dialogs (`Page.javascriptDialogOpening` /
  `Page.handleJavaScriptDialog`).
- Permissions fail closed: Chromium auto-denies prompts unless
  pre-granted, and this driver never pre-grants, matching the
  deny-by-default delegate.
- Screenshots are captured on load, input, resize and dialog
  transitions (not 60 fps); interactive smoothness is the reason the
  CEF path below exists.
- `evaluate_javascript` and `list_cookies` block up to `JS_TIMEOUT`
  (60 s) while pumping events.

| Env var | Meaning |
|---|---|
| `CHROMIUM_BIN` / `CHROME_BIN` | Chromium binary override |
| `TONTOO_CHROME_NO_SANDBOX=1` | Disable the Chromium sandbox (test containers only) |
| `TONTOO_CHROME_DIR` | Managed install dir override |
| `TONTOO_CHROME_AUTOUPDATE=0` | Disable all network provisioning |
| `TONTOO_WEBENGINE_RENDERER` | Helper default: `auto`, `mock` or `chromium` (`--renderer` flag wins) |
| `TONTOO_WEBENGINE_DEBUG=1` | CDP setup markers and reader heartbeats on stderr |
| `TONTOO_E2E_TIMEOUT_SECS` | First-frame budget of `tests/chromium_cdp.rs` (default 240) |

## Chromium Auto-Update (`chrome_provision.rs`)

The helper provisions and updates its own Chrome-for-Testing build, so
TontooOS and WSL installations always run a patched Chromium without a
distro package:

- Managed dir: `~/.local/share/tontoo-webengine/chrome` on Linux
  (`%LOCALAPPDATA%/TontooWebEngine/chrome` on Windows),
  `TONTOO_CHROME_DIR` overrides. Layout: `VERSION`, one dir per
  version, `LAST_UPDATE_CHECK`, cached milestone list.
- First run without a binary: download current Stable, smoke-test it
  (`chrome --version` must exit 0 -- this rejects builds needing a
  newer glibc than the host provides, e.g. old WSL containers), then
  walk older milestones newest-first until one runs. Only runnable
  builds are ever installed; Disk use is one version at a time.
- Every helper start refreshes at most once a day in a background
  thread; the running build keeps serving, the new one activates on
  the next start. `TONTOO_CHROME_AUTOUPDATE=0` disables everything.
- WSL note: no distro package needed. If the container glibc predates
  current Stable, the ladder automatically settles on the newest
  runnable milestone for that machine.

End-to-end proof (helper binary plus system Chromium required):

```bash
TONTOO_WEBENGINE_BIN=../../target/debug/tontoo-webengine cargo test --test process
CHROMIUM_BIN=/path/to/chrome cargo test --test chromium_cdp
```

The second test navigates a data URL, asserts `1+1 == 2` and
`document.title == "Hello"` through the full stack, and round-trips
the cookie jar.

## CEF OSR (Planned Production Path)

For 60 fps interactive pages (video, smooth scroll) the helper gains a
third renderer on the Chromium Embedded Framework in windowless mode
(`cef-rs` crate), keeping the same protocol, so the view, TontooUI
embedding and C ABI stay untouched:

- `CefWindowInfo::SetAsWindowless` plus `CefRenderHandler::OnPaint`:
  BGRA pixels flow into the existing `frame.bgra` slot (later zero-copy
  via DMA-BUF on Linux, D3D11 handles on Windows).
- Input through `CefBrowserHost::SendXXX` driven by the existing
  `MouseDown/Up/Move`, `Scroll`, `KeyText` and `SpecialKey` commands.
- `cef::do_message_loop_work()` pumped from the helper main loop next
  to the stdin/command pump.
- Needs the shared CEF binaries at build time (`CEF_PATH`, see
  `cef-rs`); the renderer stays behind a `cef` cargo feature so default
  builds keep working without the SDK. WPE WebKit remains a possible
  alternative renderer behind the same trait.

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
- [Ffi.md](Ffi.md) – C API (`webkit_vello.h`)
- [Settings.md](Settings.md) – engine settings
