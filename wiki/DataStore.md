# DataStore

`WebsiteDataType` selects which website data kinds to clear, the
equivalent of `WKWebsiteDataStore` in Apple WebKit. Clearing runs through
the engine (`WebView::clear_data`).

## Clearing

```rust
pub fn clear_data(&self, cookies: bool, cache: bool)
```

Clears cookies and/or cache in the engine store.

### `WebsiteDataType`

A struct of booleans selecting which data kinds to clear. Use the helpers
or compose one:

| Helper | Contains |
|---|---|
| `WebsiteDataType::all()` | Everything |
| `WebsiteDataType::none()` | Nothing |
| `WebsiteDataType::cookies()` | Cookies only |
| `WebsiteDataType::caches()` | Memory, disk, offline and DOM caches |

## Cross References

- [Configuration.md](Configuration.md) -- selecting the store per web view
- [WebView.md](WebView.md) -- the view clearing data through the engine
