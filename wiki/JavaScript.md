# JavaScript

TontooWebKit exposes JavaScript execution and result mapping on top of the
WebKitGTK JavaScriptCore bridge.

## Evaluating Scripts

### `WebView::evaluate_javascript`

```rust
pub fn evaluate_javascript(&self, script: &str) -> Result<serde_json::Value, WebKitError>
```

Runs `script` in the page and returns the result as `serde_json::Value`.
Returns `Err(WebKitError::Javascript)` when the script throws or the
engine reports an error. The call blocks on the default GLib main context,
so it must run on the UI thread.

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
let result = web_view.evaluate_javascript("1 + 2").unwrap();
assert_eq!(result, 3);
```

## Value Mapping

The engine returns a `javascriptcore6::Value`; the framework converts it to
JSON (`json::jsc_value_to_json`):

| JS value | JSON |
|---|---|
| `null`, `undefined` | `null` |
| boolean | `true` / `false` |
| number | integer or float |
| string | string |
| array | array |
| plain object | object |
| anything else | its string form |

Whole numbers within the `i64` range are serialized without a decimal
point.

## Message Callbacks

Script messages posted from the page arrive through
`ScriptMessageHandler` closures or `WebViewDelegate::script_message`. The
payload goes through the same value mapping. See
[ScriptMessages.md](ScriptMessages.md).

## Cross References

- [ScriptMessages.md](ScriptMessages.md) -- page-to-Rust message bridge
- [WebView.md](WebView.md) -- the widget exposing evaluation
