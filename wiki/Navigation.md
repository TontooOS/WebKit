# Navigation

Navigation covers the page load lifecycle and policy decisions, the
equivalent of `WKNavigationDelegate` in Apple WebKit. Two delegate traits
are involved:

- `WebNavigationDelegate` -- navigation events and policy decisions.
- `WebViewDelegate` -- state observations (title, url, progress, load,
  script messages).

## WebNavigationDelegate

```rust
pub trait WebNavigationDelegate {
    fn navigation_started(&mut self, url: Option<&str>) {}
    fn navigation_redirected(&mut self, url: Option<&str>) {}
    fn navigation_committed(&mut self, url: Option<&str>) {}
    fn navigation_finished(&mut self, url: Option<&str>) {}
    fn navigation_failed(&mut self, url: Option<&str>, error: &str) {}
    fn decide_policy(&mut self, url: Option<&str>, action: PolicyAction) -> bool { true }
}
```

Every method has a default implementation. `decide_policy` returns `true`
to allow a navigation and `false` to cancel it; the default allows
everything.

### `PolicyAction`

| Variant | Meaning |
|---|---|
| `Navigation` | A main-frame navigation |
| `NewWindow` | A `window.open` or target link |
| `Response` | A sub-resource response |

## WebViewDelegate

```rust
pub trait WebViewDelegate {
    fn title_changed(&mut self, title: Option<&str>) {}
    fn url_changed(&mut self, url: Option<&str>) {}
    fn load_progress(&mut self, progress: f64) {}
    fn load_started(&mut self, url: Option<&str>) {}
    fn load_finished(&mut self, url: Option<&str>) {}
    fn load_failed(&mut self, url: Option<&str>, error: &str) {}
    fn script_message(&mut self, name: &str, body: serde_json::Value) {}
    fn ready_to_show(&mut self) {}
}
```

`script_message` is the delegate-level alternative to
`ScriptMessageHandler` closures. See [ScriptMessages.md](ScriptMessages.md).

## Usage

```rust,no_run
use webkit::{NavigationAction, PolicyAction, WebKitConfiguration, WebNavigationDelegate, WebView};

struct MyNavigation;

impl WebNavigationDelegate for MyNavigation {
    fn navigation_started(&mut self, url: Option<&str>) {
        println!("loading {url:?}");
    }
    fn decide_policy(&mut self, url: Option<&str>, action: PolicyAction) -> bool {
        // Block all new windows.
        action != PolicyAction::NewWindow
    }
}

let web_view = WebView::new(WebKitConfiguration::new().start_url("https://example.com")).unwrap();
web_view.set_navigation_delegate(Box::new(MyNavigation));
```

## Behavior Notes

- `window.open()` and target links load into the same web view by default
  (the engine's `create` signal returns the current widget). Override
  `decide_policy` for `PolicyAction::NewWindow` to reject or reroute them.
- Load failures are reported only through `navigation_failed` /
  `load_failed`; the `LoadEvent::Failed` state is synthesized from the
  engine's failure signal because WebKitGTK has no separate "failed" load
  event.

## Cross References

- [WebView.md](WebView.md) -- the widget that drives both delegates
- [ScriptMessages.md](ScriptMessages.md) -- message handler callbacks
