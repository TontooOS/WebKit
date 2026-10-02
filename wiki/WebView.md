# WebView

The `WebView` is the core widget of TontooWebKit. It renders web content
and exposes navigation, history, zoom, JavaScript evaluation and state
observation, mirroring Apple's `WKWebView`.

> **Note:** this page describes the backend-neutral view (default
> `vello` feature), which exposes frames via `poll_frame` instead of a
> toolkit widget. See [Backend.md](Backend.md) for the engine split.

## Constructors

### `WebView::new`

```rust
pub fn new(config: WebKitConfiguration) -> Result<WebView, WebKitError>
```

Creates a web view from a configuration. The configuration is consumed
because script message handlers own their callbacks. Returns
`Err(WebKitError::InvalidUrl)` when the configured start URL is not a
supported scheme (`http`, `https`, `file`, `data`, `about`).

### `WebView::builder`

```rust
pub fn builder() -> WebViewBuilder
```

Fluent builder with the same options as `WebKitConfiguration`:

```rust,no_run
use webkit::{WebView, AutoPlay, WebSettings};

let web_view = WebView::builder()
    .start_url("https://example.com")
    .settings(WebSettings::builder().auto_play(AutoPlay::RequireUserGesture).build())
    .build()
    .expect("invalid start URL");
```

## Navigation

| Method | Behavior |
|---|---|
| `load_url(&self, url: &str) -> Result<(), WebKitError>` | Loads a URL; rejects unsupported schemes |
| `load_html(&self, html: &str, base_uri: Option<&str>)` | Loads raw HTML |
| `go_back(&self)` | Navigates back in history |
| `go_forward(&self)` | Navigates forward in history |
| `can_go_back(&self) -> bool` | Whether history has a previous page |
| `can_go_forward(&self) -> bool` | Whether history has a next page |
| `reload(&self)` | Reloads the current page |
| `reload_bypass_cache(&self)` | Reloads ignoring caches |
| `stop_loading(&self)` | Stops the current load |

## State

| Method | Behavior |
|---|---|
| `url(&self) -> Option<String>` | Current page URL (`None` before load) |
| `title(&self) -> Option<String>` | Current page title |
| `is_loading(&self) -> bool` | Whether a page is loading |
| `estimated_progress(&self) -> f64` | Load progress in `0.0..=1.0` |
| `zoom_level(&self) -> f64` | Current zoom (1.0 = 100%) |
| `set_zoom_level(&self, level: f64)` | Sets the zoom level |

## JavaScript

### `WebView::evaluate_javascript`

```rust
pub fn evaluate_javascript(&self, script: &str) -> Result<JsonValue, WebKitError>
```

Runs JavaScript in the page and returns the result as a Foundation
`JsonValue`. Returns `Err(WebKitError::Javascript)` when the script fails.
Blocks while pumping engine events. Use it during startup or from the C
FFI.

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
let title = web_view.evaluate_javascript("document.title").unwrap();
```

## Delegates

- `WebView::set_delegate(Box<dyn WebViewDelegate>)` receives state changes
  (title, url, progress, load events, script messages), JavaScript dialog
  requests (`script_dialog`) and permission requests
  (`permission_request`, denied by default). See
  [DialogsAndPermissions.md](DialogsAndPermissions.md).
- `WebView::set_navigation_delegate(Box<dyn WebNavigationDelegate>)`
  receives navigation events and policy decisions.
- `WebView::set_download_delegate(Box<dyn DownloadDelegate>)` decides
  where downloads are saved; without it every download is cancelled. See
  [Downloads.md](Downloads.md).

## URL Validation

`load_url` only accepts `http://`, `https://`, `file://`, `data:` and
`about:` URLs; everything else (notably `javascript:` URIs) returns
`Err(WebKitError::InvalidUrl)`.

## Cookies

`WebView::list_cookies()` / `add_cookie()` / `delete_cookie()` /
`clear_data()` talk to the engine cookie store. See
[Cookies.md](Cookies.md).

## Extensions and Tabs

| Method | Behavior |
|---|---|
| `install_extension(&self, path: &Path) -> Result<String, WebKitError>` | Installs a `manifest.json` folder or `.xpi`, returns the Firefox extension id |
| `list_extensions(&self) -> Result<Vec<String>, WebKitError>` | Installed extension ids |
| `new_tab(&self, kind: &str)` | Opens a `"tab"` or `"window"` |
| `close_tab(&self, context: &str)` | Closes a top-level context |
| `activate_tab(&self, context: &str)` | Focuses a context |
| `contexts(&self) -> Vec<String>` | Top-level contexts the engine reported |
| `engine_window(&self) -> Option<(u32, bool)>` | The engine's own window as `(pid, kiosk)` |

`install_extension` and `list_extensions` block while pumping engine
events, up to `JS_TIMEOUT`. See [Extensions.md](Extensions.md).

## Engine Window

Gecko draws into its own Wayland window and never produces a
`SharedFrame`, so `poll_frame()` returns `None` on a Gecko-backed view.
`engine_window()` reports the Firefox process id instead; look it up in
`CoreWindows::list_windows` to position or close it. See
[Gecko.md](Gecko.md).

## Cross References

- [Configuration.md](Configuration.md) -- start URL and data store
- [Settings.md](Settings.md) -- engine settings
- [Navigation.md](Navigation.md) -- navigation and view delegates
- [JavaScript.md](JavaScript.md) -- evaluation details and value mapping
- [Geolocation.md](Geolocation.md) -- page geolocation via CoreLocation
- [Extensions.md](Extensions.md) -- WebExtensions on Gecko
- [Gecko.md](Gecko.md) -- the Firefox backend
