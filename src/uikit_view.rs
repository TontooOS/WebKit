//! Legacy UIKit embedding shim.
//!
//! The `uikit` crate no longer exists (TontooUI is its successor), so this
//! module only compiles with the `gtk-backend` feature and no longer
//! implements any UIKit trait. It keeps the `WebViewContent` name alive so
//! GTK codebases keep building.
//!
//! New code targets [`crate::tontooui_view::WebViewContent`] (feature
//! `vello`) instead.

#![cfg(feature = "gtk-backend")]

use crate::config::WebKitConfiguration;
use crate::error::WebKitError;
use crate::web_view::WebView;

/// Legacy GTK wrapper kept under its historic name.
///
/// Wraps a GTK [`WebView`] and exposes its widget. Deprecated: use the
/// backend-neutral [`crate::view::WebView`] with
/// [`crate::tontooui_view::WebViewContent`] for TontooUI windows.
#[deprecated(
    since = "26.1.0",
    note = "use tontooui_view::WebViewContent with the vello feature instead"
)]
pub struct WebViewContent {
    inner: WebView,
}

#[allow(deprecated)]
impl WebViewContent {
    /// Create the wrapper from a configuration.
    pub fn new(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        Ok(Self {
            inner: WebView::new(config)?,
        })
    }

    /// The wrapped [`WebView`].
    pub fn web_view(&self) -> &WebView {
        &self.inner
    }

    /// The underlying GTK4 widget.
    pub fn widget(&self) -> gtk::Widget {
        self.inner.widget()
    }
}
