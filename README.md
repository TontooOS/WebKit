# TontooWebKit

Web content framework for TontooOS. An Apple-WebKit API for embedding a browser in your app.

The production engine is **Gecko**: headful Firefox driven over WebDriver
BiDi. Pages render in their own Wayland window at full frame rate, and
every Firefox WebExtension works (uBlock Origin, Bitwarden, Vimium, ...).
The host app draws the browser chrome in TontooUI.

```rust,no_run
use std::sync::Arc;
use webkit::{GeckoEngine, GeckoOptions, WebKitConfiguration, WebView};

let engine = GeckoEngine::launch(GeckoOptions::default()).expect("Firefox");
let view = WebView::with_engine(
    WebKitConfiguration::new().start_url("https://example.com"),
    Arc::new(engine),
).expect("web view");
view.load_url("https://example.com").expect("valid URL");
```

Try the browser (TontooUI toolbar window plus headful Firefox):

```bash
cargo run --example tontoo_browser
```

## Documentation

The full documentation lives in the wiki: [wiki/MAIN.md](wiki/MAIN.md),
[wiki/Gecko.md](wiki/Gecko.md), [wiki/Extensions.md](wiki/Extensions.md)
and [wiki/Backend.md](wiki/Backend.md).

## Made for TontooOS

Explore more at https://github.com/TontooOS/Libs

## Adding to Your Project

Add to your `Cargo.toml`:

```toml
[dependencies]
sdk = { path = "/Library/System/sdk", features = ["WebKit"] }
```

## License

TCL v26.1
