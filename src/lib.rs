//! # TontooWebKit
//!
//! Web content framework for TontooOS. Follows Apple's WebKit design
//! philosophy: a [`WebView`] widget, a [`WebKitConfiguration`] object that
//! carries settings such as the start URL, user scripts, JavaScript message
//! handlers and the website data store, plus navigation and lifecycle
//! delegates.
//!
//! The rendering backend is WebKitGTK (the Apple WebKit engine ported to
//! GTK4), so TontooOS apps get Safari-class rendering without any Chromium
//! code.
//!
//! ## Quick Start
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
//!     // web_view.widget() returns a gtk::Widget that can be added to any
//!     // GTK4 container or embedded in a UIKit view.
//! }
//! ```
//!
//! UIKit apps embed a web view through [`WebViewContent`], which implements
//! `uikit::view::ViewContent`:
//!
//! ```rust,no_run
//! use uikit::prelude::*;
//! use webkit::{WebKitConfiguration, WebViewContent};
//!
//! let content = WebViewContent::new(
//!     WebKitConfiguration::new().start_url("https://example.com"),
//! ).expect("failed to create web view");
//!
//! let view = View::new(content).with_frame(0.0, 0.0, 800.0, 600.0);
//! ```

pub mod config;
pub mod data_store;
pub mod delegate;
pub mod error;
pub mod ffi;
pub mod json;
pub mod lang;
pub mod navigation;
pub mod script;
pub mod settings;
pub mod uikit_view;
pub mod web_view;

pub use config::{DataStoreKind, WebKitConfiguration};
pub use data_store::{WebsiteData, WebsiteDataType, WebsiteDataStore};
pub use delegate::WebViewDelegate;
pub use error::WebKitError;
pub use navigation::{NavigationAction, NavigationEvent, PolicyAction, WebNavigationDelegate};
pub use script::{ScriptFrameInjection, ScriptInjectionTime, ScriptMessageHandler, WebScript};
pub use settings::{AutoPlay, CacheModel, WebSettings, WebSettingsBuilder};
pub use uikit_view::WebViewContent;
pub use web_view::{WebView, WebViewBuilder};

/// Version of the TontooWebKit framework (major, minor, patch).
pub const WEBKIT_VERSION: (u32, u32, u32) = (26, 1, 0);

/// Convenience re-exports for a single `use webkit::prelude::*;`.
pub mod prelude {
    pub use crate::{
        AutoPlay, CacheModel, DataStoreKind, NavigationAction, NavigationEvent, PolicyAction,
        ScriptFrameInjection, ScriptInjectionTime, ScriptMessageHandler, WebKitConfiguration,
        WebKitError, WebNavigationDelegate, WebScript, WebSettings, WebView, WebViewBuilder,
        WebViewContent, WebViewDelegate, WebsiteData, WebsiteDataType, WebsiteDataStore,
    };
    pub use crate::WEBKIT_VERSION;
}