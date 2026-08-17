//! The configuration object applied when a [`crate::WebView`] is created,
//! the equivalent of `WKWebViewConfiguration` in Apple WebKit.

use crate::script::{ScriptMessageHandler, WebScript};
use crate::settings::WebSettings;

/// Where website data lives for the web view.
#[derive(Debug, Clone)]
pub enum DataStoreKind {
    /// The shared, persistent default data store.
    Default,
    /// A private, ephemeral data store (private browsing).
    Ephemeral,
    /// A persistent data store rooted at custom directories.
    Custom {
        data_directory: String,
        cache_directory: String,
    },
}

/// Build-time configuration for a [`crate::WebView`].
///
/// ```rust,no_run
/// use webkit::{AutoPlay, DataStoreKind, WebKitConfiguration, WebScript, WebSettings};
///
/// let config = WebKitConfiguration::new()
///     .start_url("https://example.com")
///     .settings(WebSettings::builder().auto_play(AutoPlay::RequireUserGesture).build())
///     .user_script(WebScript::new("window.tontoo = true;").at_document_start())
///     .private_browsing(true);
/// ```
pub struct WebKitConfiguration {
    /// URL loaded when the web view is created.
    pub start_url: Option<String>,
    /// Engine settings applied to the new web view.
    pub settings: WebSettings,
    /// User scripts injected into loaded pages.
    pub user_scripts: Vec<WebScript>,
    /// JavaScript-to-Rust message channels.
    pub message_handlers: Vec<ScriptMessageHandler>,
    /// Which data store backs this web view.
    pub data_store: DataStoreKind,
}

impl WebKitConfiguration {
    /// An empty configuration with default settings and the default data
    /// store.
    pub fn new() -> Self {
        Self {
            start_url: None,
            settings: WebSettings::new(),
            user_scripts: Vec::new(),
            message_handlers: Vec::new(),
            data_store: DataStoreKind::Default,
        }
    }

    /// Set the start URL loaded on creation.
    pub fn start_url(mut self, url: impl Into<String>) -> Self {
        self.start_url = Some(url.into());
        self
    }

    /// Set the start URL, overwriting any previous value.
    pub fn set_start_url(&mut self, url: impl Into<String>) {
        self.start_url = Some(url.into());
    }

    /// Replace the engine settings.
    pub fn settings(mut self, settings: WebSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Add a user script.
    pub fn user_script(mut self, script: WebScript) -> Self {
        self.user_scripts.push(script);
        self
    }

    /// Add a JavaScript message handler.
    pub fn add_message_handler(mut self, handler: ScriptMessageHandler) -> Self {
        self.message_handlers.push(handler);
        self
    }

    /// Use a specific data store kind.
    pub fn data_store(mut self, kind: DataStoreKind) -> Self {
        self.data_store = kind;
        self
    }

    /// Enable or disable private browsing.
    pub fn private_browsing(mut self, enabled: bool) -> Self {
        self.data_store = if enabled {
            DataStoreKind::Ephemeral
        } else {
            DataStoreKind::Default
        };
        self
    }
}

impl Default for WebKitConfiguration {
    fn default() -> Self {
        Self::new()
    }
}