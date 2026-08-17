# DataStore

`WebsiteDataStore` controls where cookies, caches and storage live and can
clear stored website data, the equivalent of `WKWebsiteDataStore` in Apple
WebKit.

## Constructors

| Method | Behavior |
|---|---|
| `WebsiteDataStore::default()` | Shared persistent store |
| `WebsiteDataStore::ephemeral()` | Private browsing; nothing persists |
| `WebsiteDataStore::with_directories(data, cache)` | Persistent store at custom paths |

## Queries

| Method | Behavior |
|---|---|
| `is_ephemeral(&self) -> bool` | Whether the store is private |

## Clearing

| Method | Behavior |
|---|---|
| `clear(&self, types, time_span) -> Result<(), WebKitError>` | Clears the given types within a time span |
| `clear_all(&self) -> Result<(), WebKitError>` | Clears everything for all time |
| `clear_cookies(&self) -> Result<(), WebKitError>` | Clears cookies |
| `clear_caches(&self) -> Result<(), WebKitError>` | Clears caches |

All clearing methods block until the engine finishes. They return
`Err(WebKitError::Engine)` when the operation fails or is cancelled.

### `WebsiteDataType`

A struct of booleans selecting which data kinds to clear. Use the helpers
or compose one:

| Helper | Contains |
|---|---|
| `WebsiteDataType::all()` | Everything |
| `WebsiteDataType::none()` | Nothing |
| `WebsiteDataType::cookies()` | Cookies only |
| `WebsiteDataType::caches()` | Memory, disk, offline and DOM caches |

```rust,no_run
use std::time::Duration;
use webkit::{WebsiteDataStore, WebsiteDataType};

let store = WebsiteDataStore::default();
store.clear(WebsiteDataType::cookies(), Duration::from_days(7)).unwrap();
store.clear_all().unwrap();
```

> **Note:** `clear` uses the engine's raw `webkit_website_data_manager_clear`
> FFI because the Rust bindings in webkit6 0.4 do not expose it. The
> callback bridge is implemented in `data_store.rs` and needs no `unsafe`
> outside that module.

## Cross References

- [Configuration.md](Configuration.md) -- selecting the store per web view
- [WebView.md](WebView.md) -- the widget created with a store
