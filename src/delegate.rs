//! View-level state callbacks, the equivalent of combining
//! `WKUIDelegate` and the `WKWebView` KVO notifications in Apple WebKit.

/// Trait for observing web view state changes.
///
/// All methods have default implementations, so implementing the trait only
/// requires overriding the callbacks you actually use.
pub trait WebViewDelegate {
    /// The page title changed.
    fn title_changed(&mut self, _title: Option<&str>) {}

    /// The current URL changed.
    fn url_changed(&mut self, _url: Option<&str>) {}

    /// Loading progress changed (0.0 ..= 1.0).
    fn load_progress(&mut self, _progress: f64) {}

    /// The view started loading a page.
    fn load_started(&mut self, _url: Option<&str>) {}

    /// The view finished loading a page.
    fn load_finished(&mut self, _url: Option<&str>) {}

    /// Loading failed.
    fn load_failed(&mut self, _url: Option<&str>, _error: &str) {}

    /// A page posted a message on one of the registered script channels.
    fn script_message(&mut self, _name: &str, _body: serde_json::Value) {}

    /// The web content is ready to be shown.
    fn ready_to_show(&mut self) {}
}

/// Default delegate used when the caller does not provide one.
#[derive(Debug, Default)]
pub struct DefaultWebViewDelegate;

impl WebViewDelegate for DefaultWebViewDelegate {}