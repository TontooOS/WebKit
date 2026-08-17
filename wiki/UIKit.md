# UIKit

TontooWebKit embeds in UIKit apps through `WebViewContent`, which
implements `uikit::view::ViewContent`. Add it to a view tree exactly like
any other UIKit widget.

## WebViewContent

```rust
pub fn new(config: WebKitConfiguration) -> Result<WebViewContent, WebKitError>
pub fn web_view(&self) -> &WebView
```

`WebViewContent` owns a `WebView`. Its `render` implementation returns the
web view's GTK4 widget, so UIKit places and sizes it like a normal view.

## Example

```rust,no_run
use uikit::prelude::*;
use webkit::{WebKitConfiguration, WebViewContent};

fn main() {
    let mut app = App::new("My App", 800, 600);
    app.set_color_scheme(ColorScheme::Dark);

    let web = WebViewContent::new(
        WebKitConfiguration::new().start_url("https://example.com"),
    ).expect("failed to create web view");

    let view = View::new(web).with_frame(0.0, 0.0, 800.0, 600.0);
    app.set_root_view(view);
    app.run();
}
```

## Demo Browser

The repository ships a demo browser app built on UIKit + TontooWebKit:

```bash
cargo run --example browser
```

The example in `examples/browser.rs` shows:

- A UIKit `App` with `ColorScheme::Dark`.
- A `WebViewContent` embedded in the window.
- A GTK4 toolbar with back / forward / reload / address entry whose signal
  handlers capture the (non-`Send`) web view directly.

GTK4 widgets are used for the toolbar because UIKit's `Button` closures
require `Send + Sync`, which a GTK-bound web view cannot satisfy. The
embedding itself stays fully UIKit-native.

## Cross References

- [WebView.md](WebView.md) -- the widget behind the content
- [Configuration.md](Configuration.md) -- configuring the embedded view
