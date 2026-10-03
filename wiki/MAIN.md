# TontooWebKit -- Wiki

TontooWebKit is the web content framework for TontooOS. It follows Apple's
WebKit design philosophy with a `WebView` widget, a `WebKitConfiguration`
object (start URL, settings, user scripts, message handlers, data store),
navigation and view delegates, and a C FFI for non-Rust consumers. The
production engine is **WPE WebKit**: the page renders offscreen through the
libwpe FDO backend and arrives as BGRA frames that `WebViewContent` blits as a
Vello texture, so the browser is a single TontooUI window. There is no GTK
dependency in the stack. Headful Firefox over WebDriver BiDi stays available
behind the optional `gecko` feature for WebExtensions, but it owns its own
window.

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
| WPE | [WPE.md](WPE.md) | Offscreen WPE WebKit: FDO backend, frames, build deps |
| Extensions | [Extensions.md](Extensions.md) | WebExtensions install, list, profiles, signing (Gecko only) |
| Gecko | [Gecko.md](Gecko.md) | Optional Firefox driver: profile, launch, BiDi, auto-update |
| FFI | [Ffi.md](Ffi.md) | C API: `webkit_vello.h` |
| Backend | [Backend.md](Backend.md) | Engine trait, WPE vs Gecko vs mock, cargo features |

## Quick Start

```rust,no_run
use webkit::{WebKitConfiguration, WebViewContent};

fn main() {
    // Real WebKit, rendered offscreen and blitted into this window.
    let web = WebViewContent::with_wpe(
        WebKitConfiguration::new().start_url("https://example.com"),
    ).expect("failed to start WPE WebKit");

    // `web` is a tontooui::View: place it anywhere in a view tree.
}
```

Engine handle without the TontooUI wrapper:

```rust,no_run
use std::sync::Arc;
use webkit::{WebKitConfiguration, WebView, WpeEngine, WpeOptions};

fn main() {
    let engine = WpeEngine::launch(WpeOptions::default()).expect("WPE");
    let web_view = WebView::with_engine(
        WebKitConfiguration::new().start_url("https://example.com"),
        Arc::new(engine),
    ).expect("failed to create web view");

    web_view.load_url("https://example.com").expect("valid URL");
    web_view.pump_events();
    let frame = web_view.poll_frame();
}
```

Hosts without the WPE libraries get the placeholder engine:

```rust,no_run
use webkit::{MockEngine, WebKitConfiguration, WebView};

let view = WebView::builder()
  .start_url("https://example.com")
  .build_on(std::sync::Arc::new(MockEngine::new(800, 600)))
  .expect("invalid start URL");
let frame = view.poll_frame().expect("engine serves frames");
```

See [WPE.md](WPE.md), [Backend.md](Backend.md) and the `tontoo_browser`
example for the full browser.

## Architecture

```
WebKitConfiguration (start URL, settings, scripts, handlers, data store)
  |
  +-- WebView (backend-neutral, view.rs)
  |     +-- WebEngine trait (engine.rs: WpeEngine, MockEngine, ProcessEngine)
  |     |     +-- WpeEngine (src/wpe.rs + src/wpe/tontoo_wpe.c:
  |     |     |     libwpe FDO backend, surfaceless EGL, offscreen WebKit)
  |     |     +-- GeckoEngine (src/gecko.rs: Firefox + WebDriver BiDi, optional)
  |     |     +-- tontoo-webengine helper (mock renderer or GeckoRenderer)
  |     +-- SharedFrame (BGRA pixels -> Vello texture via WebViewContent)
  |     +-- WebViewDelegate / WebNavigationDelegate / DownloadDelegate
  |     +-- Vello C ABI (ffi_vello.rs, Headers/webkit_vello.h)
  |
  +-- WebSettings / WebScript / ScriptMessageHandler
  +-- TontooUI: WebViewContent (tontooui::View, texture blit + input)
```

## Performance Notes

- WPE renders through the same libwpe/FDO path a normal WPE embedder uses and
  composites into the host window through the texture blit, so the page is
  limited by the host's paint loop but shares one surface with the window
  chrome -- no second window, no compositor round trip.
- The engine owns one worker thread that pumps the GLib main loop in 8 ms
  slices, drains its event ring and copies the newest frame once per pump.
  Only the newest frame is kept, so a slow UI never queues up frames.
- Gecko, when enabled, renders in its own window at the compositor's frame
  rate, so page animation, WebGL and video are not limited by the host's paint
  loop. The host process only exchanges JSON.
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
- WPE needs the engine packages at build and run time
  (`wpewebkit`, `libwpe`, `wpebackend-fdo`, EGL). Without them the crate
  still builds, but `WebViewContent::with_wpe` fails and the placeholder
  engine is used instead. See [WPE.md](WPE.md).
- WPE has no add-on system, so `install_extension` and `list_extensions`
  need the `gecko` feature. See [Extensions.md](Extensions.md).
- The Gecko engine window cannot be embedded into a TontooUI window: the
  compositor implements neither `xdg_toplevel.set_parent` nor
  `xdg-foreign-v2`, and Firefox has no offscreen rendering API. The
  browser chrome is a separate, draggable TontooUI window. See
  [Gecko.md](Gecko.md).
- Chrome-less fullscreen (`--kiosk`) is **off by default** in the Gecko
  example: it sends an `xdg_toplevel.fullscreen` request, and a compositor
  that confirms with a `0 x 0` size aborts Firefox. TontooCompositor does
  not implement `fullscreen_request` yet. See [Gecko.md](Gecko.md).
- `WebDriver::stopLoading` has no BiDi equivalent, so `stop_loading` calls
  `window.stop()` and the load event still completes.
- Firefox auto-provisioning is Linux-only; on Windows and macOS the
  system Firefox is used. See [Gecko.md](Gecko.md).
- Unsigned sideloaded extensions need an ESR or DevEdition build;
  official release builds enforce AMO signatures. See
  [Extensions.md](Extensions.md).

## Changelog

- 2026-10-03: WPE WebKit replaces Gecko as the default engine --
  `src/wpe.rs` plus the C shim `src/wpe/tontoo_wpe.{c,h}` render real WebKit
  offscreen through `wpe_fdo_initialize_for_egl_display` and
  `wpe_view_backend_exportable_fdo_egl_create` on a surfaceless EGL display,
  copy the exported `wl_shm_buffer` frames to BGRA and publish them as
  `SharedFrame`, so `WebViewContent` blits the page into a single TontooUI
  window. New `wpe` cargo feature (default), `WebViewContent::with_wpe` and
  `with_engine`, `WpeEngine`/`WpeOptions`, `build.rs` resolving
  `wpe-webkit-2.0`, `wpe-1.0`, `wpebackend-fdo-1.0`, `glib-2.0`, `egl` and
  `wayland-server` through pkg-config, `tests/wpe_osr.rs` proving a painted
  640x480 frame end to end without a display server, and the
  `tontoo_browser` example rewritten as the single-window WPE browser.
  `examples/tontoo_browser.rs` moved to `examples/gecko_browser.rs`
  (needs `--features gecko`).
- 2026-10-03: Firefox crash fix -- `GeckoOptions::kiosk` now defaults to
  off and `window_size` was added. `--kiosk` sends an
  `xdg_toplevel.fullscreen` request; a compositor answering it with a
  `0 x 0` size aborts Firefox with `xdg_surface buffer (1 x 1) is larger
  than the configured fullscreen state (0 x 0)` (reproduced on WSLg).
  Forced `gfx.webrender.*` prefs were dropped so Firefox picks WebRender
  or software GL itself, every BiDi call now reports
  `GeckoPage::exit_reason()` plus the crash-report directory instead of
  `closed connection`, and `GeckoOptions::extensions` /
  `write_extension_policy` install add-ons through the profile policy
  because Firefox 140 ESR has no BiDi `webExtension` module.
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
