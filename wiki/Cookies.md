# Cookies

Cookie management for TontooWebKit, the equivalent of `WKHTTPCookieStore`
in Apple WebKit. The `CookieManager` controls which cookies the engine
accepts, reads and writes individual cookies and selects the persistent
storage format.

## Rules

- The manager is obtained per view (`WebView::cookie_manager`); views that
  share a network session share one cookie store.
- `set_accept_policy` applies immediately to every request of the session.
- The read/write methods block on the GLib main context; call them from the
  main thread only.
- Cookie persistence is part of the data store: ephemeral (private)
  sessions never write cookies to disk. See [DataStore.md](DataStore.md).

## Constructors

### `WebView::cookie_manager`

```rust
pub fn cookie_manager(&self) -> Option<CookieManager>
```

Returns the cookie manager of the view's network session, or `None` when
the engine has no session attached yet.

## API

### `CookieManager::set_accept_policy`

```rust
pub fn set_accept_policy(&self, policy: CookieAcceptPolicy)
```

Sets which cookies are accepted. Applies immediately.

### `CookieAcceptPolicy`

| Variant | Behavior |
|---|---|
| `Always` | Accept every cookie |
| `NoThirdParty` | Reject third-party cookies |
| `Never` | Reject all cookies |

### `CookieManager::accept_policy`

```rust
pub fn accept_policy(&self) -> Result<CookieAcceptPolicy, WebKitError>
```

Returns the current accept policy. Returns `Err` when the engine call
fails.

### `CookieManager::all_cookies`

```rust
pub fn all_cookies(&self) -> Result<Vec<Cookie>, WebKitError>
```

Returns every cookie in the store. Returns `Err` when the engine call
fails.

### `CookieManager::cookies_for_uri`

```rust
pub fn cookies_for_uri(&self, uri: &str) -> Result<Vec<Cookie>, WebKitError>
```

Returns all cookies that would be sent for a URI.

### `CookieManager::add_cookie`

```rust
pub fn add_cookie(&self, cookie: &Cookie) -> Result<(), WebKitError>
```

Adds or updates a cookie. Cookies without an expiry are session cookies.

### `CookieManager::delete_cookie`

```rust
pub fn delete_cookie(&self, domain: &str, path: &str, name: &str) -> Result<(), WebKitError>
```

Deletes the cookie matching domain, path and name.

### `CookieManager::set_persistent_storage`

```rust
pub fn set_persistent_storage(&self, filename: &str, storage: CookieStorage)
```

Stores cookies in the given file. Call it before the first web view of the
session is created so every cookie is persisted.

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
use webkit::{Cookie, CookieAcceptPolicy, CookieStorage, WebKitConfiguration, WebView};

let web_view = WebView::new(
    WebKitConfiguration::new().start_url("https://example.com"),
).unwrap();

if let Some(cookies) = web_view.cookie_manager() {
    cookies.set_accept_policy(CookieAcceptPolicy::NoThirdParty);
    cookies.set_persistent_storage("/tmp/cookies.txt", CookieStorage::Text);

    let _ = cookies.add_cookie(Cookie::new("session", "abc", "example.com"));
    for cookie in cookies.all_cookies().unwrap_or_default() {
        println!("{}={}", cookie.name, cookie.value);
    }
}
```

## Cross References

- [DataStore.md](DataStore.md) -- where cookies live, private browsing,
  clearing website data
- [WebView.md](WebView.md) -- creating views
