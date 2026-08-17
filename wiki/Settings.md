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

| Variant | Behavior |
|---|---|
| `DocumentViewer` | Minimal caching |
| `WebBrowser` | Standard browser caching |
| `PrimaryWebBrowser` | Aggressive caching |

## Usage

```rust,no_run
use webkit::{AutoPlay, CacheModel, WebSettings};

let settings = WebSettings::builder()
    .javascript_enabled(true)
    .webgl_enabled(true)
    .auto_play(AutoPlay::RequireUserGesture)
    .cache_model(CacheModel::WebBrowser)
    .default_font_family("SF Pro Display")
    .build();
```

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
