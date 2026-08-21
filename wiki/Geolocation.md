# Geolocation

Geolocation support for TontooWebKit. Pages request the position through
the Geolocation API; the app grants or denies the request through the
permission delegate, and positions come from CoreLocation (GPS / WiFi /
IP / timezone providers) instead of the system Geoclue service, which
TontooOS does not ship.

## Rules

- Call `attach_core_location` **once, before the first web view is
  created**. Calling it again has no effect.
- Attaching the provider never grants anything by itself: every page
  still needs a `PermissionDecision::Grant` from
  `WebViewDelegate::permission_request` (`PermissionKind::Geolocation`),
  otherwise the request is denied.
- Positions are fetched on a background thread and delivered to the
  engine on the main thread; the UI never blocks.
- Update cadence: every 2 seconds with high accuracy enabled, otherwise
  every 30 seconds.

## API

### `attach_core_location`

```rust
pub fn attach_core_location() -> Result<(), WebKitError>
```

Feeds engine geolocation from CoreLocation. Idempotent. Returns
`Err(WebKitError::Engine)` when the engine has no default web context or
no geolocation manager.

The engine drives the flow: when a page starts watching the position, the
provider starts fetching; when the page stops, fetching stops.

## Usage / Example

```rust,no_run
use webkit::{
    attach_core_location, PermissionDecision, PermissionKind,
    WebViewDelegate, WebKitConfiguration, WebView,
};

struct LocationDelegate;

impl WebViewDelegate for LocationDelegate {
    fn permission_request(&mut self, kind: PermissionKind) -> PermissionDecision {
        match kind {
            PermissionKind::Geolocation => PermissionDecision::Grant,
            _ => PermissionDecision::Deny,
        }
    }
}

fn main() {
    attach_core_location().expect("failed to attach CoreLocation");

    let web_view = WebView::new(
        WebKitConfiguration::new().start_url("https://example.com"),
    ).unwrap();
    web_view.set_delegate(Box::new(LocationDelegate));
}
```

> **Note:** The first fix can take several seconds when only network-based
> providers (WiFi/IP) are available. Pages receive no position until then.

## Cross References

- [DialogsAndPermissions.md](DialogsAndPermissions.md) -- granting
  geolocation per page
- [WebView.md](WebView.md) -- installing delegates
