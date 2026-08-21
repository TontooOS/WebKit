# Settings

`WebSettings` holds the engine settings applied to every new web view, the
equivalent of the preferences in Apple's `WKWebViewConfiguration`. The
struct is serializable so settings can be stored, sent over the C FFI as
JSON or persisted as app preferences.

## Fields

| Field | Type | Default | Description |
|---|---|---|---|
| `user_agent` | `Option<String>` | `None` | Custom user agent (`None` = engine default) |
| `javascript_enabled` | `bool` | `true` | Whether JavaScript runs |
| `developer_extras` | `bool` | `false` | Inspector shortcuts |
| `webgl_enabled` | `bool` | `true` | WebGL support |
| `webaudio_enabled` | `bool` | `true` | WebAudio support |
| `media_enabled` | `bool` | `true` | Media playback |
| `media_stream_enabled` | `bool` | `false` | Camera/microphone APIs |
| `fullscreen_enabled` | `bool` | `true` | Fullscreen playback |
| `back_forward_navigation_gestures` | `bool` | `true` | Swipe navigation |
| `javascript_can_open_windows` | `bool` | `false` | `window.open` permission |
| `allow_modal_dialogs` | `bool` | `true` | JS alert/confirm/prompt |
| `auto_play` | `AutoPlay` | `Allow` | Media autoplay policy |
| `cache_model` | `CacheModel` | `WebBrowser` | Cache aggressiveness |
| `page_cache` | `bool` | `true` | Keeps rendered pages in memory for instant back/forward |
| `smooth_scrolling` | `bool` | `true` | Animated smooth scrolling |
| `dns_prefetching` | `bool` | `true` | Prefetches DNS for links on the page |
| `hardware_acceleration` | `bool` | `true` | GL-composited rendering (`Always` policy) |
| `default_font_family` | `Option<String>` | `None` | Default HTML font |
| `default_font_size` | `Option<u32>` | `None` | Default font size in px |
| `disable_web_security` | `bool` | `false` | Disables same-origin policy |

## Enums

### `AutoPlay`

| Variant | Behavior |
|---|---|
| `Allow` | Media may play without interaction |
| `RequireUserGesture` | Media only plays after user interaction |
| `AllowSilent` | Muted media autoplays; unmuted needs a gesture |

### `CacheModel`

The cache model controls how much decoded content (images, resources,
rendered pages) the engine keeps in memory. It is applied on the shared
web context when a web view is created, so every view of the process
shares one cache pool.

| Variant | Behavior |
|---|---|
| `DocumentViewer` | Minimal caching; images are re-fetched and re-decoded per page |
| `WebBrowser` | Standard browser caching |
| `PrimaryWebBrowser` | Maps to the engine's most aggressive mode (`WebBrowser`) |

> **Note:** WebKitGTK only offers three cache models. `PrimaryWebBrowser`
> is kept for API compatibility with Apple WebKit and maps to the same
> engine value as `WebBrowser`.

## Usage

```rust,no_run
use webkit::{AutoPlay, CacheModel, WebSettings};

let settings = WebSettings::builder()
    .javascript_enabled(true)
    .webgl_enabled(true)
    .auto_play(AutoPlay::RequireUserGesture)
    .cache_model(CacheModel::WebBrowser)
    .page_cache(true)
    .smooth_scrolling(true)
    .dns_prefetching(true)
    .hardware_acceleration(true)
    .default_font_family("SF Pro Display")
    .build();
```

## Performance Defaults

The defaults are tuned for a browser-class app. For slow devices or
embedded builds the performance switches can be relaxed individually:

| Switch | Effect when disabled |
|---|---|
| `cache_model = DocumentViewer` | Lowest memory use; slow repeat visits |
| `page_cache = false` | Back/forward re-renders pages from scratch |
| `dns_prefetching = false` | No background DNS lookups for links |
| `hardware_acceleration = false` | Software compositing; slower scrolling and video |
| `smooth_scrolling = false` | Instant (non-animated) scroll steps |

## Serialization

Missing fields deserialize to their defaults, so a partial JSON object is
accepted:

```json
{
  "javascript_enabled": true,
  "auto_play": "require_user_gesture"
}
```

## Cross References

- [Configuration.md](Configuration.md) -- how settings are attached
- [WebView.md](WebView.md) -- `engine_settings()` for engine extras
- [Ffi.md](Ffi.md) -- settings inside the JSON config
