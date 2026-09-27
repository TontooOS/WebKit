# Cookies

Cookie management for TontooWebKit, the equivalent of `WKHTTPCookieStore`
in Apple WebKit. Cookies live in the engine store; the view reads and
writes them through blocking calls.

## Rules

- `CookieAcceptPolicy` selects which cookies the engine accepts.
- The read/write calls block while pumping engine events; call them from
  a worker thread, not the UI thread.
- Cookie persistence is part of the data store: ephemeral (private)
  sessions never write cookies to disk. See [DataStore.md](DataStore.md).

## API

### `WebView::list_cookies`

```rust
pub fn list_cookies(&self) -> Result<Vec<Cookie>, WebKitError>
```

Returns every cookie in the store. Returns `Err` when the engine call
fails.

### `WebView::add_cookie`

```rust
pub fn add_cookie(&self, cookie: &Cookie)
```

Adds or updates a cookie. Cookies without an expiry are session cookies.

### `WebView::delete_cookie`

```rust
pub fn delete_cookie(&self, domain: &str, path: &str, name: &str)
```

Deletes the cookie matching domain, path and name.

### `CookieAcceptPolicy`

| Variant | Behavior |
|---|---|
| `Always` | Accept every cookie |
| `NoThirdParty` | Reject third-party cookies |
| `Never` | Reject all cookies |

### `CookieStorage`

| Variant | Behavior |
|---|---|
| `CookieStorage::Text` | Human-readable text file |
| `CookieStorage::Sqlite` | SQLite database |

## The `Cookie` Struct

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Cookie name |
| `value` | `String` | Cookie value |
| `domain` | `String` | Domain the cookie belongs to |
| `path` | `String` | Path the cookie belongs to (default `/`) |
| `secure` | `bool` | Only sent over secure connections |
| `http_only` | `bool` | Hidden from JavaScript (`HttpOnly`) |

Builder methods: `Cookie::new(name, value, domain)`, `.path(...)`,
`.secure(...)`, `.http_only(...)`.

## Usage / Example

```rust,no_run
use webkit::{Cookie, WebKitConfiguration, WebView};

let web_view = WebView::new(
    WebKitConfiguration::new().start_url("https://example.com"),
).unwrap();

web_view.add_cookie(&Cookie::new("session", "abc", "example.com"));
for cookie in web_view.list_cookies().unwrap_or_default() {
    println!("{}={}", cookie.name, cookie.value);
}
web_view.delete_cookie("example.com", "/", "session");
```

## Cross References

- [DataStore.md](DataStore.md) -- where cookies live, private browsing,
  clearing website data
- [WebView.md](WebView.md) -- creating views
