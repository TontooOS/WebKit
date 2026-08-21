//! Per-web-view engine settings, the equivalent of `WKWebViewConfiguration`
//! prefs in Apple WebKit.
//!
//! A [`WebSettings`] value is serializable so it can be stored, passed
//! through the C FFI as JSON, or persisted as an app preference.

use serde::{Deserialize, Serialize};
use webkit6 as wk;

/// How automatic media playback is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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

    /// Map the cache model onto the engine's context-level cache model.
    ///
    /// The engine applies the cache model on the shared web context.
    /// `CacheModel::PrimaryWebBrowser` maps to the engine's most aggressive
    /// model (`WebBrowser`), which is the strongest mode WebKitGTK offers.
    pub(crate) fn engine_cache_model(&self) -> wk::CacheModel {
        match self.cache_model {
            CacheModel::DocumentViewer => wk::CacheModel::DocumentViewer,
            CacheModel::WebBrowser | CacheModel::PrimaryWebBrowser => wk::CacheModel::WebBrowser,
        }
    }

    pub(crate) fn apply_to(&self, s: &wk::Settings) {
        s.set_enable_javascript(self.javascript_enabled);
        s.set_enable_developer_extras(self.developer_extras);
        s.set_enable_webgl(self.webgl_enabled);
        s.set_enable_webaudio(self.webaudio_enabled);
        s.set_enable_media(self.media_enabled);
        s.set_enable_media_stream(self.media_stream_enabled);
        s.set_enable_fullscreen(self.fullscreen_enabled);
        s.set_enable_back_forward_navigation_gestures(self.back_forward_navigation_gestures);
        s.set_javascript_can_open_windows_automatically(self.javascript_can_open_windows);
        s.set_allow_modal_dialogs(self.allow_modal_dialogs);
        s.set_user_agent(self.user_agent.as_deref());
        s.set_disable_web_security(self.disable_web_security);

        // Performance-relevant engine switches.
        s.set_enable_page_cache(self.page_cache);
        s.set_enable_smooth_scrolling(self.smooth_scrolling);
        s.set_enable_dns_prefetching(self.dns_prefetching);
        s.set_hardware_acceleration_policy(if self.hardware_acceleration {
            wk::HardwareAccelerationPolicy::Always
        } else {
            wk::HardwareAccelerationPolicy::Never
        });

        match self.auto_play {
            AutoPlay::Allow => {}
            AutoPlay::RequireUserGesture => {
                s.set_media_playback_requires_user_gesture(true);
            }
            AutoPlay::AllowSilent => {
                s.set_media_playback_allows_inline(true);
                s.set_media_playback_requires_user_gesture(true);
            }
        }

        if let Some(family) = &self.default_font_family {
            s.set_default_font_family(family);
        }
        if let Some(size) = self.default_font_size {
            s.set_default_font_size(size);
        }
    }
}

impl Default for WebSettings {
    fn default() -> Self {
        Self::new()
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