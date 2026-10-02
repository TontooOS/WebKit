//! # TontooWebKit
//!
//! Web content framework for TontooOS. Follows Apple's WebKit design
//! philosophy: a [`WebView`] widget, a [`WebKitConfiguration`] object that
//! carries settings such as the start URL, user scripts, JavaScript message
//! handlers and the website data store, plus navigation and lifecycle
//! delegates.
//!
//! ## Backends
//!
//! * Default (`vello` feature): backend-neutral [`view::WebView`] driving a
//!   [`engine::WebEngine`]. The `tontoo-webengine` helper runs headful
//!   Firefox and speaks WebDriver BiDi to it
//!   ([`gecko::GeckoPage`]), so pages render in their own Wayland window
//!   with WebExtensions enabled; [`engine::MockEngine`] stands in on hosts
//!   without Firefox.
//! * Engines that produce pixel buffers publish them as
//!   [`engine::SharedFrame`], which
//!   [`tontooui_view::WebViewContent`] blits as a Vello texture, so rounded
//!   corners and LiquidGlass keep working in TontooUI windows.
//!
//! ## Quick Start (Gecko)
//!
//! ```rust,no_run
//! use webkit::{GeckoOptions, WebEngine, WebView, WebKitConfiguration};
//!
//! fn main() {
//!     let engine = webkit::GeckoEngine::launch(GeckoOptions::default())
//!         .expect("failed to launch Firefox");
//!
//!     let view = WebView::with_engine(
//!     WebKitConfiguration::new().start_url("https://tontoo-os.github.io"),
//!     std::sync::Arc::new(webkit::GeckoEngine::launch(
//!         webkit::GeckoOptions::default(),
//!     ).expect("failed to launch Firefox")),
//! ).expect("failed to create web view");
//! view.load_url("https://tontoo-os.github.io").unwrap();
//! }
//! ```
//!
//! ## Quick Start (Vello)
//!
//! ```rust,no_run
//! use webkit::{WebKitConfiguration, WebView};
//!
//! fn main() {
//!     let config = WebKitConfiguration::new()
//!         .start_url("https://tontoo-os.github.io")
//!         .private_browsing(true);
//!
//!     let web_view = WebView::new(config).expect("failed to create web view");
//!     web_view.load_url("https://tontoo-os.github.io");
//!
//!     // web_view.poll_frame() returns the latest SharedFrame; blit it
//!     // as a Vello texture, or embed WebViewContent in a TontooUI tree.
//! }
//! ```
//!
//! TontooUI apps embed a web view through [`tontooui_view::WebViewContent`],
//! which implements `tontooui::elements::layout::View`:
//!
//! ```rust,no_run
//! use tontooui::elements::layout::{View, VStack};
//! use webkit::{WebKitConfiguration, WebViewContent};
//!
//! let content = WebViewContent::new(
//!     WebKitConfiguration::new().start_url("https://example.com"),
//! ).expect("failed to create web view");
//!
//! let stack = VStack::new().child(content);
//! ```

pub mod config;
pub mod cookie;
pub mod data_store;
pub mod delegate;
pub mod download;
pub mod engine;
pub mod error;
#[cfg(feature = "vello")]
pub mod ffi_vello;
#[cfg(feature = "gecko")]
pub mod gecko;
#[cfg(feature = "gecko")]
pub mod gecko_provision;
pub mod geolocation;
pub mod lang;
pub mod navigation;
pub mod script;
pub mod settings;
#[cfg(feature = "vello")]
pub mod tontooui_view;
pub mod transport;
pub use transport::{ProcessEngine, find_helper};
#[cfg(feature = "vello")]
pub mod view;

pub use config::{DataStoreKind, WebKitConfiguration};
pub use cookie::{Cookie, CookieAcceptPolicy, CookieStorage};
pub use data_store::{WebsiteData, WebsiteDataType};
pub use delegate::{
    DefaultWebViewDelegate, PermissionDecision, PermissionKind, ScriptDialogKind, ScriptDialogRef,
    WebViewDelegate,
};
pub use download::{DefaultDownloadDelegate, DownloadDelegate, WebDownload};
pub use engine::{EngineCommand, EngineEvent, MockEngine, SharedFrame, WebEngine};
pub use error::WebKitError;
#[cfg(feature = "gecko")]
pub use gecko::{
    BidiEvent, ExtensionPolicy, GeckoEngine, GeckoOptions, GeckoPage, find_firefox,
    write_extension_policy,
};
#[cfg(feature = "gecko")]
pub use gecko_provision::{Channel, ensure_firefox};
pub use geolocation::attach_core_location;
pub use navigation::{NavigationAction, NavigationEvent, PolicyAction, WebNavigationDelegate};
pub use script::{ScriptFrameInjection, ScriptInjectionTime, ScriptMessageHandler, WebScript};
pub use settings::{AutoPlay, CacheModel, WebSettings, WebSettingsBuilder};
#[cfg(feature = "vello")]
pub use tontooui_view::WebViewContent;
#[cfg(feature = "vello")]
pub use view::{WebView, WebViewBuilder};

/// Version of the TontooWebKit framework (major, minor, patch).
pub const WEBKIT_VERSION: (u32, u32, u32) = (26, 1, 0);

/// Convenience re-exports for a single `use webkit::prelude::*;`.
pub mod prelude {
    pub use crate::{
        attach_core_location, AutoPlay, CacheModel, Cookie, CookieAcceptPolicy, CookieStorage,
        DataStoreKind, DefaultDownloadDelegate, DefaultWebViewDelegate, DownloadDelegate,
        EngineCommand, EngineEvent, MockEngine, NavigationAction, NavigationEvent,
        PermissionDecision, PermissionKind, PolicyAction, ScriptDialogKind, ScriptDialogRef,
        ScriptFrameInjection, ScriptInjectionTime, ScriptMessageHandler, SharedFrame, WebDownload,
        WebEngine, WebKitConfiguration, WebKitError, WebNavigationDelegate, WebScript, WebSettings,
        WebSettingsBuilder,
    };
    pub use crate::{WebsiteData, WebsiteDataType};
    #[cfg(feature = "gecko")]
    pub use crate::{
        BidiEvent, Channel, ExtensionPolicy, GeckoEngine, GeckoOptions, GeckoPage, find_firefox,
        write_extension_policy,
    };
    #[cfg(feature = "vello")]
    pub use crate::{WebView, WebViewBuilder, WebViewContent};
    pub use crate::WEBKIT_VERSION;
}
