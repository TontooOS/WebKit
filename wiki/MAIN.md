# TontooWebKit -- Wiki

TontooWebKit is the web content framework for TontooOS. It follows Apple's
WebKit design philosophy with a `WebView` widget, a `WebKitConfiguration`
object (start URL, settings, user scripts, message handlers, data store),
navigation and view delegates, and a C FFI for non-Rust consumers. The
production engine is **Gecko**: headful Firefox driven over WebDriver
BiDi, so pages render in their own Wayland window at full frame rate with
WebExtensions. Engines that produce pixel buffers blit into TontooUI as a
Vello texture through `WebViewContent`. There is no GTK dependency in the
stack.

- Repository: tontoo-os/TontooLibs/WebKit
- License: MIT
- Version: 27.0.0

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
| Extensions | [Extensions.md](Extensions.md) | WebExtensions install, list, profiles, signing |
| Gecko | [Gecko.md](Gecko.md) | Firefox driver: profile, launch, BiDi mapping, auto-update |
| FFI | [Ffi.md](Ffi.md) | C API: `webkit_vello.h` |
| Backend | [Backend.md](Backend.md) | Engine trait, Gecko vs helper, cargo features, IPC |

## Quick Start

```rust,no_run
use std::sync::Arc;
use webkit::{GeckoEngine, GeckoOptions, WebKitConfiguration, WebView};

fn main() {
    // Headful Firefox with WebExtensions; Firefox owns the window.
    let engine = GeckoEngine::launch(GeckoOptions::default()).expect("Firefox");
    let web_view = WebView::with_engine(
        WebKitConfiguration::new().start_url("https://example.com"),
        Arc::new(engine),
    ).expect("failed to create web view");

    web_view.load_url("https://example.com").expect("valid URL");
    web_view.pump_events();
    let _title = web_view.title();
}
```

Offline hosts (no Firefox) get the test engine instead:

```rust,no_run
use webkit::{MockEngine, WebKitConfiguration, WebView};

let view = WebView::builder()
  .start_url("https://example.com")
  .build_on(std::sync::Arc::new(MockEngine::new(800, 600)))
  .expect("invalid start URL");
let frame = view.poll_frame().expect("engine serves frames");
```

See [Gecko.md](Gecko.md), [Backend.md](Backend.md) and the
`tontoo_browser` example for the full browser.

## Architecture

```
WebKitConfiguration (start URL, settings, scripts, handlers, data store)
  |
  +-- WebView (backend-neutral, view.rs)
  |     +-- WebEngine trait (engine.rs: MockEngine, GeckoEngine, ProcessEngine)
  |     |     +-- GeckoPage (src/gecko.rs: Firefox + WebDriver BiDi)
  |     |     +-- tontoo-webengine helper (mock renderer or GeckoRenderer)
  |     +-- SharedFrame (BGRA pixels -> Vello texture via WebViewContent)
  |     +-- WebViewDelegate / WebNavigationDelegate / DownloadDelegate
  |     +-- Vello C ABI (ffi_vello.rs, Headers/webkit_vello.h)
  |
  +-- WebSettings / WebScript / ScriptMessageHandler
  +-- TontooUI: WebViewContent (tontooui::View, texture blit + input)
```

## Performance Notes

- Gecko renders in its own window at the compositor's frame rate, so page
  animation, WebGL and video are not limited by the host's paint loop.
  The host process only exchanges JSON.
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
- The `cdylib` and `rlib` targets share one code base; the FFI layer is only
  active in the C-facing build.

## Known Limitations

- Private browsing is decided when a web view is created
  (`WebKitConfiguration::private_browsing`). Switching a live view between
  the ephemeral and the persistent session at runtime is **not supported
  yet** -- recreate the view instead. See [DataStore.md](DataStore.md).
- The engine window cannot be embedded into a TontooUI window: the
  compositor implements neither `xdg_toplevel.set_parent` nor
  `xdg-foreign-v2`, and Firefox has no offscreen rendering API. The
  browser chrome is a separate, draggable TontooUI window. See
  [Gecko.md](Gecko.md).
- `WebDriver::stopLoading` has no BiDi equivalent, so `stop_loading` calls
  `window.stop()` and the load event still completes.
- Firefox auto-provisioning is Linux-only; on Windows and macOS the
  system Firefox is used. See [Gecko.md](Gecko.md).
- Unsigned sideloaded extensions need an ESR or DevEdition build;
  official release builds enforce AMO signatures. See
  [Extensions.md](Extensions.md).

## Changelog

- 2026-10-02: Gecko replaces Chromium -- `src/gecko.rs` launches headful
  Firefox and speaks WebDriver BiDi (`browsingContext`, `script`,
  `input`, `storage`, `network`, `log`, `webExtension`), `src/gecko.rs`
  adds the in-process `GeckoEngine`, `src/gecko_provision.rs` downloads
  and auto-updates a Firefox ESR build (Linux, `lzma-rs` + `tar`, 1x/day
  in the background, `TONTOO_FIREFOX_*` env table). New protocol variants
  `InstallExtension`, `ListExtensions`, `NewTab`, `CloseTab`,
  `ActivateTab` and the events `EngineWindow`, `ContextReady`,
  `ContextClosed`, `ExtensionInstalled`, `ExtensionFailed`, `Extensions`;
  `WebView::install_extension`, `list_extensions`, `new_tab`, `close_tab`,
  `activate_tab`, `contexts`, `engine_window`;
  `WebViewDelegate::extension_installed`; `Cookie::expires`.
  `chromium.rs`, `chrome_provision.rs` and `tests/chromium_cdp.rs` are
  removed, the `chromium` feature is replaced by `gecko`, and
  `examples/tontoo_browser.rs` (TontooUI toolbar window + kiosk Firefox)
  replaces the screenshot demos as the browser entry point.
  `tests/gecko_bidi.rs` proves `1 + 1 == 2` end to end against a headless
  private Firefox.
- 2026-09-26: Chromium auto-update -- `chrome_provision.rs` downloads
  and smoke-tests Chrome-for-Testing (newest-first ladder, glibc-safe),
  daily background updates, managed dir, `TONTOO_CHROME_*` env table;
  PATH binaries are smoke-tested; CDP launch hardened (HTTP/1.1 with
  Content-Length reads, target polling, no background networking).
- 2026-09-26: Chromium renderer -- headless Chromium over CDP
  (`chromium.rs`, `chromium` feature on by default): real pages in the
  Chromium sandbox, screenshots, input, JavaScript, cookies, downloads,
  dialogs; `SpecialKey` command plus `History` event; Vello
  `key_press` C function; `tests/chromium_cdp.rs` proves `1+1 == 2`
  end to end. CEF OSR documented as the 60 fps production path.
- 2026-09-26: Helper discovery fix -- `find_helper` walks up from the
  executable (cargo `examples/`/`deps/` layouts), so the demos find the
  helper after one `cargo build`. `cargo run --example` alone does not
  build the helper binary.
- 2026-09-27: GTK backend removed -- `gtk-backend` feature, GTK widgets,
  `CookieManager`, `WebsiteDataStore`, GTK FFI and UIKit shim deleted;
  Vello/TontooUI is the only backend. Engine IPC (`EngineCommand` /
  `EngineEvent`) and CDP messages use Foundation JSON (no serde).
- 2026-09-26: Full Vello loop -- `tontoo-webengine` helper (line
  protocol, frame files, `--ping` self test), `ProcessEngine` transport
  with helper discovery and mock fallback, JS/cookie id correlation with
  5 s timeout, dialog/permission/download answers through the delegates,
  Vello C ABI (`Headers/webkit_vello.h`, `ffi_vello.rs`), `vello_browser`
  demo (toolbar, address field, progress, status), `tests/process.rs`
  round-trip.
- 2026-09-26: Vello backend split -- backend-neutral `WebView`
  (`WebEngine` trait, `SharedFrame`, `EngineCommand`/`EngineEvent`,
  `MockEngine`), `WebViewContent` for TontooUI texture blit, `vello`
  (default) cargo feature, `vello_webview` example. WPE helper process
  is planned, not shipped.

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
