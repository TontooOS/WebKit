//! Per-web-view engine settings, the equivalent of `WKWebViewConfiguration`
//! prefs in Apple WebKit.

/// How automatic media playback is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoPlay {
    /// Media may play without any user interaction.
    #[default]
    Allow,
    /// Media only plays after the user interacts with the page.
    RequireUserGesture,
    /// Muted media may play; unmuted media requires a user gesture.
    AllowSilent,
}

/// How aggressively the engine caches web content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheModel {
    /// Minimal caching (document viewers, single-page apps).
    DocumentViewer,
    /// Standard browser caching.
    #[default]
    WebBrowser,
    /// Most aggressive caching (frequently visited sites).
    PrimaryWebBrowser,
}

/// Mutable engine settings applied to every new [`crate::WebView`].
#[derive(Debug, Clone)]
pub struct WebSettings {
    /// Custom user agent string. `None` lets the engine pick a default.
    pub user_agent: Option<String>,
    /// Whether JavaScript is enabled. Defaults to `true`.
    pub javascript_enabled: bool,
    /// Whether developer extras (inspector shortcuts) are enabled.
    pub developer_extras: bool,
    /// Whether WebGL is enabled. Defaults to `true`.
    pub webgl_enabled: bool,
    /// Whether WebAudio is enabled. Defaults to `true`.
    pub webaudio_enabled: bool,
    /// Whether media (audio/video) playback is enabled. Defaults to `true`.
    pub media_enabled: bool,
    /// Whether the media stream (camera/microphone) APIs are enabled.
    pub media_stream_enabled: bool,
    /// Whether the engine allows fullscreen playback.
    pub fullscreen_enabled: bool,
    /// Whether swipe back/forward gestures are enabled.
    pub back_forward_navigation_gestures: bool,
    /// Whether `window.open` from JavaScript is allowed.
    pub javascript_can_open_windows: bool,
    /// Whether modal JavaScript dialogs are allowed.
    pub allow_modal_dialogs: bool,
    /// Automatic media playback policy.
    pub auto_play: AutoPlay,
    /// Cache model.
    pub cache_model: CacheModel,
    /// Whether the page cache keeps rendered pages in memory for instant
    /// back/forward navigation. Defaults to `true`.
    pub page_cache: bool,
    /// Whether scrolling is animated smoothly. Defaults to `true`.
    pub smooth_scrolling: bool,
    /// Whether the engine prefetches DNS for links on the page. Defaults to
    /// `true`.
    pub dns_prefetching: bool,
    /// Whether the view composites through hardware acceleration (GL).
    /// Defaults to `true`.
    pub hardware_acceleration: bool,
    /// Default font family for HTML content.
    pub default_font_family: Option<String>,
    /// Default font size in pixels.
    pub default_font_size: Option<u32>,
    /// Whether web security (same-origin policy) is disabled.
    pub disable_web_security: bool,
}

impl WebSettings {
    /// Settings with recommended defaults for TontooOS apps.
    pub fn new() -> Self {
        Self {
            user_agent: None,
            javascript_enabled: true,
            developer_extras: false,
            webgl_enabled: true,
            webaudio_enabled: true,
            media_enabled: true,
            media_stream_enabled: false,
            fullscreen_enabled: true,
            back_forward_navigation_gestures: true,
            javascript_can_open_windows: false,
            allow_modal_dialogs: true,
            auto_play: AutoPlay::Allow,
            cache_model: CacheModel::WebBrowser,
            page_cache: true,
            smooth_scrolling: true,
            dns_prefetching: true,
            hardware_acceleration: true,
            default_font_family: None,
            default_font_size: None,
            disable_web_security: false,
        }
    }

    /// Builder-style entry point.
    pub fn builder() -> WebSettingsBuilder {
        WebSettingsBuilder::default()
    }
}

impl Default for WebSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSettings {
    /// Parse from a JSON object. Missing fields fall back to [`WebSettings::new`]
    /// defaults (matching the former serde behavior).
    pub fn from_json_value(
        doc: &foundation::serialization::JsonValue,
    ) -> Result<Self, String> {
        use foundation::serialization::JsonValue;
        let defaults = Self::new();
        let opt_str = |key: &str| match doc.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::Str(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("field `{}` has the wrong type", key)),
        };
        let boolean = |key: &str, fallback: bool| match doc.get(key) {
            None | Some(JsonValue::Null) => Ok(fallback),
            Some(JsonValue::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("field `{}` has the wrong type", key)),
        };
        let opt_u32 = |key: &str| match doc.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("field `{}` has the wrong type", key)),
        };
        let auto_play = match doc.get("auto_play").and_then(|v| v.as_str()) {
            None => defaults.auto_play,
            Some("allow") => AutoPlay::Allow,
            Some("require_user_gesture") => AutoPlay::RequireUserGesture,
            Some("allow_silent") => AutoPlay::AllowSilent,
            Some(other) => return Err(format!("unknown auto_play `{}`", other)),
        };
        let cache_model = match doc.get("cache_model").and_then(|v| v.as_str()) {
            None => defaults.cache_model,
            Some("document_viewer") => CacheModel::DocumentViewer,
            Some("web_browser") => CacheModel::WebBrowser,
            Some("primary_web_browser") => CacheModel::PrimaryWebBrowser,
            Some(other) => return Err(format!("unknown cache_model `{}`", other)),
        };
        Ok(Self {
            user_agent: opt_str("user_agent")?.or(defaults.user_agent),
            javascript_enabled: boolean("javascript_enabled", defaults.javascript_enabled)?,
            developer_extras: boolean("developer_extras", defaults.developer_extras)?,
            webgl_enabled: boolean("webgl_enabled", defaults.webgl_enabled)?,
            webaudio_enabled: boolean("webaudio_enabled", defaults.webaudio_enabled)?,
            media_enabled: boolean("media_enabled", defaults.media_enabled)?,
            media_stream_enabled: boolean(
                "media_stream_enabled",
                defaults.media_stream_enabled,
            )?,
            fullscreen_enabled: boolean("fullscreen_enabled", defaults.fullscreen_enabled)?,
            back_forward_navigation_gestures: boolean(
                "back_forward_navigation_gestures",
                defaults.back_forward_navigation_gestures,
            )?,
            javascript_can_open_windows: boolean(
                "javascript_can_open_windows",
                defaults.javascript_can_open_windows,
            )?,
            allow_modal_dialogs: boolean("allow_modal_dialogs", defaults.allow_modal_dialogs)?,
            auto_play,
            cache_model,
            page_cache: boolean("page_cache", defaults.page_cache)?,
            smooth_scrolling: boolean("smooth_scrolling", defaults.smooth_scrolling)?,
            dns_prefetching: boolean("dns_prefetching", defaults.dns_prefetching)?,
            hardware_acceleration: boolean(
                "hardware_acceleration",
                defaults.hardware_acceleration,
            )?,
            default_font_family: opt_str("default_font_family")?,
            default_font_size: opt_u32("default_font_size")?,
            disable_web_security: boolean("disable_web_security", defaults.disable_web_security)?,
        })
    }
}

/// Fluent builder for [`WebSettings`].
#[derive(Debug, Clone, Default)]
pub struct WebSettingsBuilder {
    settings: WebSettings,
}

impl WebSettingsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn user_agent(mut self, agent: impl Into<String>) -> Self {
        self.settings.user_agent = Some(agent.into());
        self
    }

    pub fn javascript_enabled(mut self, enabled: bool) -> Self {
        self.settings.javascript_enabled = enabled;
        self
    }

    pub fn developer_extras(mut self, enabled: bool) -> Self {
        self.settings.developer_extras = enabled;
        self
    }

    pub fn webgl_enabled(mut self, enabled: bool) -> Self {
        self.settings.webgl_enabled = enabled;
        self
    }

    pub fn webaudio_enabled(mut self, enabled: bool) -> Self {
        self.settings.webaudio_enabled = enabled;
        self
    }

    pub fn media_enabled(mut self, enabled: bool) -> Self {
        self.settings.media_enabled = enabled;
        self
    }

    pub fn media_stream_enabled(mut self, enabled: bool) -> Self {
        self.settings.media_stream_enabled = enabled;
        self
    }

    pub fn fullscreen_enabled(mut self, enabled: bool) -> Self {
        self.settings.fullscreen_enabled = enabled;
        self
    }

    pub fn back_forward_navigation_gestures(mut self, enabled: bool) -> Self {
        self.settings.back_forward_navigation_gestures = enabled;
        self
    }

    pub fn javascript_can_open_windows(mut self, enabled: bool) -> Self {
        self.settings.javascript_can_open_windows = enabled;
        self
    }

    pub fn allow_modal_dialogs(mut self, enabled: bool) -> Self {
        self.settings.allow_modal_dialogs = enabled;
        self
    }

    pub fn auto_play(mut self, policy: AutoPlay) -> Self {
        self.settings.auto_play = policy;
        self
    }

    pub fn cache_model(mut self, model: CacheModel) -> Self {
        self.settings.cache_model = model;
        self
    }

    pub fn page_cache(mut self, enabled: bool) -> Self {
        self.settings.page_cache = enabled;
        self
    }

    pub fn smooth_scrolling(mut self, enabled: bool) -> Self {
        self.settings.smooth_scrolling = enabled;
        self
    }

    pub fn dns_prefetching(mut self, enabled: bool) -> Self {
        self.settings.dns_prefetching = enabled;
        self
    }

    pub fn hardware_acceleration(mut self, enabled: bool) -> Self {
        self.settings.hardware_acceleration = enabled;
        self
    }

    pub fn default_font_family(mut self, family: impl Into<String>) -> Self {
        self.settings.default_font_family = Some(family.into());
        self
    }

    pub fn default_font_size(mut self, size: u32) -> Self {
        self.settings.default_font_size = Some(size);
        self
    }

    pub fn disable_web_security(mut self, disabled: bool) -> Self {
        self.settings.disable_web_security = disabled;
        self
    }

    pub fn build(self) -> WebSettings {
        self.settings
    }
}