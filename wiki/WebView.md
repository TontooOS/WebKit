# WebView

The `WebView` is the core widget of TontooWebKit. It renders web content
and exposes navigation, history, zoom, JavaScript evaluation and state
observation, mirroring Apple's `WKWebView`.

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

## Widget Access

### `WebView::widget`

```rust
pub fn widget(&self) -> gtk::Widget
```

Returns the underlying GTK4 widget. The returned widget is a reference-
counted handle and stays valid independent of the `WebView`. Use it to add
the web view to a GTK container or a UIKit view (`WebViewContent::new`
does this automatically).

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
pub fn evaluate_javascript(&self, script: &str) -> Result<serde_json::Value, WebKitError>
```

Runs JavaScript in the page and returns the result as JSON. Returns
`Err(WebKitError::Javascript)` when the script fails. Blocks on the default
GLib main context until the engine answers, which freezes rendering for the
duration. Use it during startup or from the C FFI; prefer
`evaluate_javascript_async` while the main loop is running.

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
let title = web_view.evaluate_javascript("document.title").unwrap();
```

### `WebView::evaluate_javascript_async`

```rust
pub fn evaluate_javascript_async(
    &self,
    script: &str,
) -> Result<impl Future<Output = Result<serde_json::Value, WebKitError>>, WebKitError>
```

Non-blocking variant of `evaluate_javascript`. Returns a future that
resolves when the engine has evaluated the script; the UI thread keeps
rendering while the engine works. Spawn it on the GLib main context:

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
let future = web_view.evaluate_javascript_async("document.title").unwrap();
glib::MainContext::default().spawn_local(async move {
    if let Ok(title) = future.await {
        println!("title: {title}");
    }
});
```

Returns `Err(WebKitError::Javascript)` when the future resolves to an
engine error, mirroring the blocking variant.

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

`WebView::cookie_manager()` exposes the session's cookie store (accept
policy, read/write cookies, persistence). See [Cookies.md](Cookies.md).

## Engine Settings Access

`WebView::engine_settings()` returns the raw `webkit6::Settings` object for
engine options not exposed by `WebSettings`.

## Inspector (F12 devtools)

WebKitGTK ships a full web inspector (Elements, Console, Network, Sources,
Performance / CPU profile). Enable it with developer extras, then call
`show_inspector` / `close_inspector` / `toggle_inspector`.

| Method | Behavior |
|---|---|
| `set_developer_extras(true)` | Enables the inspector at runtime |
| `WebSettings.developer_extras = true` | Enables it at build time |
| `show_inspector()` | Opens the inspector |
| `close_inspector()` | Closes it |
| `is_inspector_open()` | Whether it is attached |
| `toggle_inspector()` | Open or close; returns the new state |

```rust,no_run
web_view.set_developer_extras(true);
web_view.toggle_inspector(); // opens the WebKit web inspector
```

> The demo browser uses **F12 to open its own performance window** (process
> CPU/MEM view), not the inspector. Use the lib methods directly to open
> WebKit's built-in inspector.
>
> The demo browser's status bar doubles as a hover-URL indicator: while the
> pointer is over a link it shows the link target (via
> `mouse-target-changed`), otherwise the current page URL. The label is
> width-capped (`EllipsizeMode::End` + `max_width_chars`), so long URLs
> ellipsize instead of resizing the window.

## Cross References

- [Configuration.md](Configuration.md) -- start URL and data store
- [Settings.md](Settings.md) -- engine settings
- [Navigation.md](Navigation.md) -- navigation and view delegates
- [JavaScript.md](JavaScript.md) -- evaluation details and value mapping
- [UIKit.md](UIKit.md) -- embedding in UIKit apps
- [Geolocation.md](Geolocation.md) -- page geolocation via CoreLocation
