//! The backend-neutral [`WebView`], the equivalent of `WKWebView` in
//! Apple WebKit.
//!
//! Unlike the legacy GTK view (`crate::web_view`, feature `gtk-backend`),
//! this view owns no widget. It drives a [`crate::engine::WebEngine`]
//! (WPE WebKit out-of-process once `tontoo-webengine` lands,
//! [`crate::engine::MockEngine`] until then) and exposes the latest
//! [`crate::engine::SharedFrame`] for texture blitting into TontooUI
//! (see `crate::tontooui_view::WebViewContent`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::config::{DataStoreKind, WebKitConfiguration};
use crate::delegate::{DefaultWebViewDelegate, WebViewDelegate};
use crate::download::{DefaultDownloadDelegate, DownloadDelegate, WebDownload};
use crate::engine::{EngineCommand, MockEngine, SharedFrame, WebEngine};
use crate::error::WebKitError;
use crate::navigation::{DefaultWebNavigationDelegate, WebNavigationDelegate};
use crate::script::ScriptMessageHandler;
use crate::settings::WebSettings;

/// A web view without a toolkit widget. Frames are pulled with
/// [`WebView::poll_frame`] and drawn as a Vello texture.
pub struct WebView {
    engine: Arc<dyn WebEngine>,
    config: WebKitConfiguration,
    delegate: Rc<RefCell<Box<dyn WebViewDelegate>>>,
    navigation: Rc<RefCell<Box<dyn WebNavigationDelegate>>>,
    downloads: Rc<RefCell<Box<dyn DownloadDelegate>>>,
    url: RefCell<Option<String>>,
    title: RefCell<Option<String>>,
    progress: RefCell<f64>,
    loading: RefCell<bool>,
    can_back: RefCell<bool>,
    can_forward: RefCell<bool>,
    zoom: RefCell<f64>,
}

impl WebView {
    /// Create a web view from a configuration, backed by the test engine.
    ///
    /// The configuration is kept (settings, scripts and message handlers
    /// are forwarded to the real engine once it connects; see
    /// [`WebView::with_engine`]).
    pub fn new(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        Self::with_engine(config, Arc::new(MockEngine::new(800, 600)))
    }

    /// Create a web view on an explicit engine (WPE helper, CEF, tests).
    pub fn with_engine(
        config: WebKitConfiguration,
        engine: Arc<dyn WebEngine>,
    ) -> Result<Self, WebKitError> {
        if let Some(url) = &config.start_url {
            crate::engine::validate_url(url)?;
        }
        let view = Self {
            engine,
            config,
            delegate: Rc::new(RefCell::new(Box::new(DefaultWebViewDelegate))),
            navigation: Rc::new(RefCell::new(Box::new(DefaultWebNavigationDelegate))),
            downloads: Rc::new(RefCell::new(Box::new(DefaultDownloadDelegate))),
            url: RefCell::new(None),
            title: RefCell::new(None),
            progress: RefCell::new(0.0),
            loading: RefCell::new(false),
            can_back: RefCell::new(false),
            can_forward: RefCell::new(false),
            zoom: RefCell::new(1.0),
        };
        if let Some(url) = view.config.start_url.clone() {
            view.load_url(&url)?;
        }
        Ok(view)
    }

    /// Fluent builder entry point.
    pub fn builder() -> WebViewBuilder {
        WebViewBuilder::new()
    }

    /// The engine driving this view.
    pub fn engine(&self) -> &Arc<dyn WebEngine> {
        &self.engine
    }

    /// The latest rendered frame, if the engine produced one yet.
    pub fn poll_frame(&self) -> Option<Arc<SharedFrame>> {
        self.engine.latest_frame()
    }

    /// Tell the engine its allocated size (physical pixels + scale).
    pub fn resize(&self, width: u32, height: u32, scale: f32) {
        self.engine.resize(width, height, scale);
    }

    /// Set the view delegate (state callbacks).
    pub fn set_delegate(&self, delegate: Box<dyn WebViewDelegate>) {
        *self.delegate.borrow_mut() = delegate;
    }

    /// Set the navigation delegate (policy and navigation callbacks).
    pub fn set_navigation_delegate(&self, delegate: Box<dyn WebNavigationDelegate>) {
        *self.navigation.borrow_mut() = delegate;
    }

    /// Set the download delegate. Without one every download is cancelled.
    pub fn set_download_delegate(&self, delegate: Box<dyn DownloadDelegate>) {
        *self.downloads.borrow_mut() = delegate;
    }

    /// Decide whether a download may be saved. Returns the destination
    /// path chosen by the delegate, or `None` to cancel.
    pub fn decide_download_destination(
        &self,
        download: &WebDownload,
        suggested_filename: &str,
    ) -> Option<String> {
        self.downloads
            .borrow_mut()
            .decide_destination(download, suggested_filename)
    }

    /// Load a URL. Only `http(s)`, `file`, `data` and `about` URLs are
    /// accepted.
    pub fn load_url(&self, url: &str) -> Result<(), WebKitError> {
        self.engine.load_url(url)?;
        *self.url.borrow_mut() = Some(url.to_string());
        *self.loading.borrow_mut() = true;
        self.navigation
            .borrow_mut()
            .navigation_started(Some(url));
        self.delegate.borrow_mut().load_started(Some(url));
        Ok(())
    }

    /// Load raw HTML content.
    pub fn load_html(&self, html: &str, base_uri: Option<&str>) {
        self.engine.load_html(html, base_uri);
        *self.loading.borrow_mut() = true;
    }

    /// Navigate back in the session history.
    pub fn go_back(&self) {
        self.engine.send(EngineCommand::GoBack);
    }

    /// Navigate forward in the session history.
    pub fn go_forward(&self) {
        self.engine.send(EngineCommand::GoForward);
    }

    /// Whether there is a previous page in the history.
    pub fn can_go_back(&self) -> bool {
        *self.can_back.borrow()
    }

    /// Whether there is a next page in the history.
    pub fn can_go_forward(&self) -> bool {
        *self.can_forward.borrow()
    }

    /// Reload the current page.
    pub fn reload(&self) {
        self.engine.send(EngineCommand::Reload);
    }

    /// Reload the current page, bypassing caches.
    pub fn reload_bypass_cache(&self) {
        self.engine.send(EngineCommand::ReloadBypassCache);
    }

    /// Stop the current load.
    pub fn stop_loading(&self) {
        self.engine.send(EngineCommand::Stop);
        *self.loading.borrow_mut() = false;
    }

    /// The current page URL, if any.
    pub fn url(&self) -> Option<String> {
        self.url.borrow().clone()
    }

    /// The current page title, if any.
    pub fn title(&self) -> Option<String> {
        self.title.borrow().clone()
    }

    /// Whether a page is currently loading.
    pub fn is_loading(&self) -> bool {
        *self.loading.borrow()
    }

    /// Estimated load progress between 0.0 and 1.0.
    pub fn estimated_progress(&self) -> f64 {
        *self.progress.borrow()
    }

    /// Current zoom level (1.0 = 100%).
    pub fn zoom_level(&self) -> f64 {
        *self.zoom.borrow()
    }

    /// Set the zoom level (1.0 = 100%). Applied by the UI scaler until
    /// the engine supports page zoom natively.
    pub fn set_zoom_level(&self, level: f64) {
        *self.zoom.borrow_mut() = level.max(0.25).min(5.0);
    }

    /// Forward a mouse press in view coordinates (logical px).
    pub fn mouse_down(&self, x: f64, y: f64) {
        self.engine.send(EngineCommand::MouseDown { x, y });
    }

    /// Forward a mouse release in view coordinates (logical px).
    pub fn mouse_up(&self, x: f64, y: f64) {
        self.engine.send(EngineCommand::MouseUp { x, y });
    }

    /// Forward a pointer move in view coordinates (logical px).
    pub fn mouse_move(&self, x: f64, y: f64) {
        self.engine.send(EngineCommand::MouseMove { x, y });
    }

    /// Forward a scroll delta in logical px (right/down positive).
    pub fn scroll(&self, dx: f64, dy: f64) {
        self.engine.send(EngineCommand::Scroll { dx, dy });
    }

    /// Forward typed text.
    pub fn input_text(&self, text: &str) {
        self.engine
            .send(EngineCommand::KeyText(text.to_string()));
    }

    /// Apply one engine event (called by the IPC pump in the reader
    /// thread or the run loop; unit tests call it directly).
    pub fn apply_event(&self, event: crate::engine::EngineEvent) {
        use crate::engine::EngineEvent as E;
        match event {
            E::Title(title) => {
                *self.title.borrow_mut() = title.clone();
                self.delegate
                    .borrow_mut()
                    .title_changed(title.as_deref());
            }
            E::Url(url) => {
                *self.url.borrow_mut() = url.clone();
                self.delegate.borrow_mut().url_changed(url.as_deref());
            }
            E::Progress(p) => {
                *self.progress.borrow_mut() = p;
                self.delegate.borrow_mut().load_progress(p);
            }
            E::LoadStarted(url) => {
                *self.loading.borrow_mut() = true;
                self.navigation
                    .borrow_mut()
                    .navigation_started(url.as_deref());
                self.delegate
                    .borrow_mut()
                    .load_started(url.as_deref());
            }
            E::LoadFinished(url) => {
                *self.loading.borrow_mut() = false;
                *self.progress.borrow_mut() = 1.0;
                self.navigation
                    .borrow_mut()
                    .navigation_finished(url.as_deref());
                self.delegate
                    .borrow_mut()
                    .load_finished(url.as_deref());
            }
            E::LoadFailed { url, error } => {
                *self.loading.borrow_mut() = false;
                self.navigation
                    .borrow_mut()
                    .navigation_failed(url.as_deref(), &error);
                self.delegate
                    .borrow_mut()
                    .load_failed(url.as_deref(), &error);
            }
            E::ScriptMessage { name, body } => {
                for handler in &self.config.message_handlers {
                    if handler.name == name {
                        (handler.body)(body.clone());
                    }
                }
                self.delegate.borrow_mut().script_message(&name, body);
            }
            E::ReadyToShow => {
                self.delegate.borrow_mut().ready_to_show();
            }
            E::FrameReady { .. } | E::JsResult { .. } => {}
        }
    }

    /// Run JavaScript in the page.
    ///
    /// The out-of-process engine answers asynchronously; this stub resolves
    /// immediately with `null` until the JS-result pump (`JsResult` events
    /// correlated by id) lands with the WPE helper.
    pub fn evaluate_javascript(
        &self,
        _script: &str,
    ) -> Result<serde_json::Value, WebKitError> {
        Ok(serde_json::Value::Null)
    }

    /// The configured settings (serializable; forwarded to the engine
    /// helper on spawn).
    pub fn settings(&self) -> &WebSettings {
        &self.config.settings
    }

    /// The configured data store kind (applied by the engine helper).
    pub fn data_store(&self) -> &DataStoreKind {
        &self.config.data_store
    }
}

/// Fluent builder for [`WebView`].
pub struct WebViewBuilder {
    config: WebKitConfiguration,
}

impl WebViewBuilder {
    pub fn new() -> Self {
        Self {
            config: WebKitConfiguration::new(),
        }
    }

    pub fn start_url(mut self, url: impl Into<String>) -> Self {
        self.config.start_url = Some(url.into());
        self
    }

    pub fn settings(mut self, settings: WebSettings) -> Self {
        self.config.settings = settings;
        self
    }

    pub fn user_script(mut self, script: crate::script::WebScript) -> Self {
        self.config.user_scripts.push(script);
        self
    }

    pub fn add_message_handler(mut self, handler: ScriptMessageHandler) -> Self {
        self.config.message_handlers.push(handler);
        self
    }

    pub fn data_store(mut self, kind: DataStoreKind) -> Self {
        self.config.data_store = kind;
        self
    }

    pub fn private_browsing(mut self, enabled: bool) -> Self {
        self.config.data_store = if enabled {
            DataStoreKind::Ephemeral
        } else {
            DataStoreKind::Default
        };
        self
    }

    /// Build on the test engine. Fails only when the start URL is invalid.
    pub fn build(self) -> Result<WebView, WebKitError> {
        WebView::new(self.config)
    }

    /// Build on an explicit engine (WPE helper, CEF, tests).
    pub fn build_on(self, engine: Arc<dyn WebEngine>) -> Result<WebView, WebKitError> {
        WebView::with_engine(self.config, engine)
    }
}

impl Default for WebViewBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_url_validation_survives() {
        let view = WebView::builder().start_url("https://example.com").build();
        assert!(view.is_ok());
        let bad = WebView::builder()
            .start_url("javascript:alert(1)")
            .build();
        assert!(bad.is_err());
    }

    #[test]
    fn events_reach_delegates() {
        use crate::engine::EngineEvent;
        use std::cell::Cell;

        struct Probe(Rc<Cell<bool>>);
        impl crate::delegate::WebViewDelegate for Probe {
            fn load_finished(&mut self, _url: Option<&str>) {
                self.0.set(true);
            }
        }

        let view = WebView::builder().build().unwrap();
        let flag = Rc::new(Cell::new(false));
        view.set_delegate(Box::new(Probe(flag.clone())));
        view.apply_event(EngineEvent::LoadFinished(None));
        assert!(flag.get());
        assert!(!view.is_loading());
    }
}
