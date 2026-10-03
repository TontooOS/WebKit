# TontooWebKit

Web content framework for TontooOS. An Apple-WebKit API for embedding a browser in your app.

The production engine is **WPE WebKit**: the page renders offscreen through
libwpe's FDO backend and the frames are blitted as a Vello texture, so the
browser is a single TontooUI window -- toolbar and page in one surface, no
engine window, no compositor round trip.

```rust,no_run
use webkit::{WebKitConfiguration, WebViewContent};

// The page is a TontooUI view.
let web = WebViewContent::with_wpe(
    WebKitConfiguration::new().start_url("https://example.com"),
).expect("WPE WebKit");
```

Try the browser (toolbar, address field, progress and page in one window):

```bash
cargo run --example tontoo_browser
```

Needs the engine packages at build and run time:

```bash
pacman -S --needed wpewebkit libwpe wpebackend-fdo
```

Headful Firefox over WebDriver BiDi is still available behind the optional
`gecko` feature for WebExtensions (uBlock Origin, Bitwarden, Vimium, ...).
Firefox owns its own window in that mode:

```bash
cargo run --example gecko_browser --features gecko
```

## Documentation

The full documentation lives in the wiki: [wiki/MAIN.md](wiki/MAIN.md),
[wiki/WPE.md](wiki/WPE.md), [wiki/Gecko.md](wiki/Gecko.md),
[wiki/Extensions.md](wiki/Extensions.md) and [wiki/Backend.md](wiki/Backend.md).

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
