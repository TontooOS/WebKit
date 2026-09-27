# JavaScript

TontooWebKit exposes JavaScript execution and result mapping on top of the
engine (Chromium CDP `Runtime.evaluate`).

## Evaluating Scripts

### `WebView::evaluate_javascript`

```rust
pub fn evaluate_javascript(&self, script: &str) -> Result<JsonValue, WebKitError>
```

Runs `script` in the page and returns the result as a Foundation
`JsonValue`. Returns `Err(WebKitError::Javascript)` when the script throws
or the engine reports an error. The call blocks while pumping engine
events, so it must not run on the UI thread.

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
let result = web_view.evaluate_javascript("1 + 2").unwrap();
assert_eq!(result, foundation::serialization::JsonValue::Integer(2));
```

## Value Mapping

The engine returns a CDP Runtime `RemoteObject`; the framework maps it to
JSON (`chromium::remote_to_json`):

| JS value | JSON |
|---|---|
| `null`, `undefined` | `null` |
| boolean | `true` / `false` |
| number | integer or float |
| string | string |
| array | array |
| plain object | object |
| non-serializable (`NaN`, infinities) | `null` |

## Message Callbacks

Script messages posted from the page arrive through
`ScriptMessageHandler` closures or `WebViewDelegate::script_message`. The
payload goes through the same value mapping. See
[ScriptMessages.md](ScriptMessages.md).

## Cross References

- [ScriptMessages.md](ScriptMessages.md) -- page-to-Rust message bridge
- [WebView.md](WebView.md) -- the widget exposing evaluation
