# TontooWebKit -- Wiki

TontooWebKit is the web content framework for TontooOS. It follows Apple's
WebKit design philosophy with a `WebView` widget, a `WebKitConfiguration`
object (start URL, settings, user scripts, message handlers, data store),
navigation and view delegates, and a C FFI for non-Rust consumers. The
default backend is engine-neutral (WPE WebKit out-of-process; mock frames
until the helper lands) and blits into TontooUI as a Vello texture, so
there is no Chromium code and no GTK dependency in the stack. The legacy
WebKitGTK backend stays available behind the `gtk-backend` cargo feature.

- Repository: tontoo-os/TontooLibs/WebKit
- License: MIT
- Version: 26.1.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| WebView | [WebView.md](WebView.md) | The web view widget, navigation, JavaScript |
| Configuration | [Configuration.md](Configuration.md) | Build-time config, start URL, data store |
| Settings | [Settings.md](Settings.md) | Engine settings (JS, media, autoplay, cache) |
| Navigation | [Navigation.md](Navigation.md) | Navigation and policy delegate |
| ScriptMessages | [ScriptMessages.md](ScriptMessages.md) | User scripts and the JS-to-Rust bridge |
| DataStore | [DataStore.md](DataStore.md) | Website data, private browsing, clearing |
| Cookies | [Cookies.md](Cookies.md) | Cookie accept policy, read/write, persistence |
| Downloads | [Downloads.md](Downloads.md) | Download delegate and save-location handling |
| DialogsAndPermissions | [DialogsAndPermissions.md](DialogsAndPermissions.md) | JS dialogs and permission requests |
| Geolocation | [Geolocation.md](Geolocation.md) | Page geolocation backed by CoreLocation |
| FFI | [Ffi.md](Ffi.md) | C API and `Headers/webkit.h` (`gtk-backend` only) |
| UIKit | [UIKit.md](UIKit.md) | Legacy GTK embedding shim (deprecated) |
| Backend | [Backend.md](Backend.md) | Engine trait, WPE plan, cargo features, IPC |

## Quick Start

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

fn main() {
    let config = WebKitConfiguration::new()
        .start_url("https://example.com")
        .private_browsing(true);

    let web_view = WebView::new(config).expect("failed to create web view");
    let frame = web_view.poll_frame();
    // blit `frame` as a Vello texture, or embed WebViewContent in TontooUI.
}
```

See [WebView.md](WebView.md), [Backend.md](Backend.md) and the
`vello_webview` example for details.

## Architecture

```
WebKitConfiguration (start URL, settings, scripts, handlers, data store)
  |
  +-- WebView (backend-neutral, view.rs)
  |     +-- WebEngine trait (engine.rs: MockEngine now, WPE helper planned)
  |     +-- SharedFrame (BGRA pixels -> Vello texture via WebViewContent)
  |     +-- WebViewDelegate / WebNavigationDelegate / DownloadDelegate
  |
  +-- WebSettings / WebScript / ScriptMessageHandler (serializable)
  +-- TontooUI: WebViewContent (tontooui::View, texture blit + input)
  |
  +-- Legacy (feature gtk-backend): GtkWebView (WebKitGTK widget),
  |     CookieManager, WebsiteDataStore, C FFI, UIKit shim
```

## Performance Notes

- The cache model is applied on the shared web context when a web view is
  created (`WebSettings::cache_model`, default `WebBrowser`). Without it the
  engine stays at its `DocumentViewer` default and re-fetches/re-decodes
  images on every page.
- Page cache, smooth scrolling, DNS prefetching and hardware-accelerated
  compositing are enabled by default; each can be turned off individually
  through `WebSettings` (see [Settings.md](Settings.md)).
- `evaluate_javascript` blocks the UI thread until the engine answers. Use
  `evaluate_javascript_async` while the main loop is running to keep
  rendering responsive.
- Script message handlers are connected through detailed GLib signals
  (`script-message-received::<name>`), so multiple handlers dispatch by name
  without a global match loop.
- The `cdylib` and `rlib` targets share one code base; the FFI layer is only
  active in the C-facing build.

## Known Limitations

- Private browsing is decided when a web view is created
  (`WebKitConfiguration::private_browsing`). Switching a live view between
  the ephemeral and the persistent session at runtime is **not supported
  yet** -- recreate the view instead. See [DataStore.md](DataStore.md).

## Changelog

- 2026-09-26: Vello backend split -- backend-neutral `WebView`
  (`WebEngine` trait, `SharedFrame`, `EngineCommand`/`EngineEvent`,
  `MockEngine`), `WebViewContent` for TontooUI texture blit, `vello`
  (default) and `gtk-backend` cargo features, GTK/FFI/cookie/data-store
  code gated behind `gtk-backend`, `uikit` dependency removed,
  `vello_webview` example. WPE helper process is planned, not shipped.

- 2026-08-21: Geolocation -- `attach_core_location` feeds page positions
  from CoreLocation (GPS/WiFi/IP) through the engine geolocation manager;
  no Geoclue needed. Permission per page still required.
- 2026-08-21: Security and completeness pass -- strict URL scheme
  allowlist in `load_url` (rejects `javascript:` and unknown schemes),
  download delegate (`DownloadDelegate`, `WebDownload`), cookie manager
  (`CookieManager`, accept policy, read/write, persistence), JS dialog
  hook (`script_dialog`) and permission-request hook
  (`permission_request`, denied by default). README links the wiki.
  Known limitation documented: no runtime incognito switching.
- 2026-08-21: Performance pass -- cache model is now applied on the web
  context, new `WebSettings` switches (`page_cache`, `smooth_scrolling`,
  `dns_prefetching`, `hardware_acceleration`) enabled by default, and
  non-blocking `WebView::evaluate_javascript_async`.
- 2026-08-21: Demo browser -- status bar shows hovered link URLs and no
  longer resizes the window on long URLs (ellipsized, width-capped).
- 2026-08-17: Initial wiki, WebKit crate, WebKitGTK backend, UIKit
  integration, FFI headers, demo browser app.
