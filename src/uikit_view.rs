//! UIKit embedding for TontooWebKit.
//!
//! [`WebViewContent`] implements `uikit::view::ViewContent`, so a web view
//! can be added to any UIKit view tree exactly like a label or a button.

use uikit::style::Rect;
use uikit::view::ViewContent;

use crate::config::WebKitConfiguration;
use crate::error::WebKitError;
use crate::web_view::WebView;

/// A UIKit-compatible wrapper around a [`WebView`].
///
/// ```rust,no_run
/// use uikit::prelude::*;
/// use webkit::{WebKitConfiguration, WebViewContent};
///
/// let web = WebViewContent::new(
///     WebKitConfiguration::new().start_url("https://example.com"),
/// ).expect("failed to create web view");
///
/// let view = View::new(web).with_frame(0.0, 0.0, 800.0, 600.0);
/// ```
pub struct WebViewContent {
    inner: WebView,
}

impl WebViewContent {
    /// Create the UIKit content wrapper from a configuration.
    pub fn new(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        Ok(Self {
            inner: WebView::new(config)?,
        })
    }

    /// The wrapped [`WebView`].
    pub fn web_view(&self) -> &WebView {
        &self.inner
    }
}

impl ViewContent for WebViewContent {
    fn render(&self, _frame: Rect) -> gtk::Widget {
        self.inner.widget()
    }
}