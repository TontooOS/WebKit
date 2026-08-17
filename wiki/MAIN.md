# TontooWebKit -- Wiki

TontooWebKit is the web content framework for TontooOS. It follows Apple's
WebKit design philosophy with a `WebView` widget, a `WebKitConfiguration`
object (start URL, settings, user scripts, message handlers, data store),
navigation and view delegates, and a C FFI for non-Rust consumers. The
rendering backend is WebKitGTK (the Apple WebKit engine on GTK4), so there
is no Chromium code in the stack.

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
| FFI | [Ffi.md](Ffi.md) | C API and `Headers/webkit.h` |
| UIKit | [UIKit.md](UIKit.md) | Embedding in UIKit apps via `WebViewContent` |

## Quick Start

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

fn main() {
    let config = WebKitConfiguration::new()
        .start_url("https://example.com")
        .private_browsing(true);

    let web_view = WebView::new(config).expect("failed to create web view");
    let widget = web_view.widget();
    // add `widget` to any GTK4 container or UIKit view.
}
```

See [WebView.md](WebView.md) and [UIKit.md](UIKit.md) for details.

## Architecture

```
WebKitConfiguration (start URL, settings, scripts, handlers, data store)
  |
  +-- WebView            (wraps WebKitGTK's WebView widget)
  |     +-- WebViewDelegate         (title, url, progress, load, script messages)
  |     +-- WebNavigationDelegate   (navigation events, policy decisions)
  |
  +-- WebSettings        (applied to the engine settings object)
  +-- WebScript / ScriptMessageHandler   (injected JS + message bridge)
  +-- WebsiteDataStore   (default / ephemeral / custom + clear)
  |
  +-- FFI                (C ABI, Headers/webkit.h)
  +-- WebViewContent     (uikit::view::ViewContent for UIKit apps)
```

## Performance Notes

- `evaluate_javascript` blocks on the default main context and reuses the
  engine's async API, so it is safe to call from the UI thread.
- Script message handlers are connected through detailed GLib signals
  (`script-message-received::<name>`), so multiple handlers dispatch by name
  without a global match loop.
- The `cdylib` and `rlib` targets share one code base; the FFI layer is only
  active in the C-facing build.

## Changelog

- 2026-08-17: Initial wiki, WebKit crate, WebKitGTK backend, UIKit
  integration, FFI headers, demo browser app.
