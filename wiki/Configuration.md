# Configuration

`WebKitConfiguration` collects everything needed to create a web view, the
equivalent of Apple's `WKWebViewConfiguration`: the start URL, engine
settings, user scripts, script message handlers and the website data store.

## Fields

| Field | Type | Description |
|---|---|---|
| `start_url` | `Option<String>` | URL loaded when the web view is created |
| `settings` | `WebSettings` | Engine settings (see [Settings.md](Settings.md)) |
| `user_scripts` | `Vec<WebScript>` | Scripts injected into loaded pages |
| `message_handlers` | `Vec<ScriptMessageHandler>` | JS-to-Rust channels |
| `data_store` | `DataStoreKind` | Where website data lives |

## Constructors

### `WebKitConfiguration::new`

```rust
pub fn new() -> Self
```

Creates a configuration with default settings, the default data store and
no scripts, handlers or start URL.

## Builder Methods

| Method | Behavior |
|---|---|
| `start_url(url: impl Into<String>)` | Sets the start URL |
| `set_start_url(&mut self, url)` | Sets the start URL in place |
| `settings(settings: WebSettings)` | Replaces the engine settings |
| `user_script(script: WebScript)` | Appends a user script |
| `add_message_handler(handler)` | Appends a script message handler |
| `data_store(kind: DataStoreKind)` | Sets the data store kind |
| `private_browsing(enabled: bool)` | Toggles the ephemeral data store |

## Data Store Kinds

`DataStoreKind` has three variants:

| Variant | Behavior |
|---|---|
| `Default` | Shared persistent store |
| `Ephemeral` | Private browsing; nothing is written to disk |
| `Custom { data_directory, cache_directory }` | Persistent store at custom paths |

```rust,no_run
use webkit::{DataStoreKind, WebKitConfiguration};

let config = WebKitConfiguration::new()
    .start_url("https://example.com")
    .data_store(DataStoreKind::Custom {
        data_directory: "/home/user/.local/share/MyApp".into(),
        cache_directory: "/home/user/.cache/MyApp".into(),
    });
```

## Example

```rust,no_run
use webkit::{AutoPlay, ScriptMessageHandler, WebKitConfiguration, WebScript, WebSettings};

let config = WebKitConfiguration::new()
    .start_url("https://example.com")
    .settings(WebSettings::builder().auto_play(AutoPlay::RequireUserGesture).build())
    .user_script(WebScript::new("window.tontoo = true;").at_document_start())
    .add_message_handler(ScriptMessageHandler::new("ready", |body| {
        println!("page says: {body}");
    }))
    .private_browsing(true);
```

## Usage

```rust,no_run
use webkit::{WebKitConfiguration, WebView};

let web_view = WebView::new(WebKitConfiguration::new().start_url("https://example.com"))
    .expect("failed to create web view");
```

The same configuration shape is used by the C FFI, serialized as JSON. See
[Ffi.md](Ffi.md).

## Cross References

- [WebView.md](WebView.md) -- the widget the configuration builds
- [Settings.md](Settings.md) -- `WebSettings`
- [ScriptMessages.md](ScriptMessages.md) -- scripts and handlers
- [DataStore.md](DataStore.md) -- `DataStoreKind` and clearing
