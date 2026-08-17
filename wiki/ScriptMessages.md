# ScriptMessages

User scripts and the JavaScript-to-Rust message bridge, the equivalent of
`WKUserScript` and `WKScriptMessageHandler` in Apple WebKit.

## WebScript

A `WebScript` is injected into every page (or only the top frame) before or
after the document is parsed.

| Field | Type | Description |
|---|---|---|
| `source` | `String` | The JavaScript source |
| `injection_time` | `ScriptInjectionTime` | `AtDocumentStart` / `AtDocumentEnd` |
| `frames` | `ScriptFrameInjection` | `TopFrame` / `AllFrames` |
| `allow_list` | `Vec<String>` | Only inject into these URLs (empty = all) |
| `block_list` | `Vec<String>` | Never inject into these URLs |

```rust,no_run
use webkit::{ScriptFrameInjection, ScriptInjectionTime, WebScript};

let script = WebScript::new("window.tontoo = true;")
    .at_document_start()
    .in_all_frames();
```

## ScriptMessageHandler

A `ScriptMessageHandler` registers a named channel. JavaScript running in
the page posts a message with:

```js
window.webkit.messageHandlers.ready.postMessage({ user: "arlo", count: 3 });
```

and the payload arrives in Rust as JSON:

```rust,no_run
use webkit::{ScriptMessageHandler, WebKitConfiguration, WebView};

let config = WebKitConfiguration::new().add_message_handler(
    ScriptMessageHandler::new("ready", |body| {
        println!("page says: {body}");
    }),
);

let web_view = WebView::new(config).unwrap();
```

### How it works

- Each handler is registered with the engine's `UserContentManager` and
  connected through the detailed GLib signal
  `script-message-received::<name>`, so handlers dispatch by name without a
  global match loop.
- The payload is a `javascriptcore6::Value` converted to JSON with the same
  mapping described in [JavaScript.md](JavaScript.md).
- If a name is registered twice, the second registration is skipped.

### Delegate alternative

Messages can also be observed from `WebViewDelegate::script_message(name,
body)`. Handler closures and the delegate can coexist.

## Cross References

- [JavaScript.md](JavaScript.md) -- evaluation and value mapping
- [WebView.md](WebView.md) -- the widget that hosts the message bridge
- [Configuration.md](Configuration.md) -- attaching scripts and handlers
