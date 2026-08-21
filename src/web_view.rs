//! The [`WebView`] widget and its builder, the equivalent of `WKWebView` in
//! Apple WebKit.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use webkit6 as wk;
use webkit6::prelude::*;

use crate::config::{DataStoreKind, WebKitConfiguration};
use crate::delegate::{
    DefaultWebViewDelegate, PermissionDecision, PermissionKind, ScriptDialogRef, WebViewDelegate,
};
use crate::download::{DefaultDownloadDelegate, DownloadDelegate, WebDownload};
use crate::error::WebKitError;
use crate::navigation::{
    load_event_to_navigation, DefaultWebNavigationDelegate, NavigationEvent, PolicyAction,
    WebNavigationDelegate,
};
use crate::script::{ScriptMessageHandler, WebScript};
use crate::settings::WebSettings;

/// A widget that displays web content. The centerpiece of the framework.
///
/// ```rust,no_run
/// use webkit::{WebKitConfiguration, WebView};
///
/// let web_view = WebView::new(
///     WebKitConfiguration::new().start_url("https://example.com"),
/// ).expect("failed to create web view");
///
/// // Get the GTK4 widget and add it to any GTK4 container:
/// let widget = web_view.widget();
/// ```
pub struct WebView {
    inner: wk::WebView,
    delegate: Rc<RefCell<Box<dyn WebViewDelegate>>>,
    navigation: Rc<RefCell<Box<dyn WebNavigationDelegate>>>,
    downloads: Rc<RefCell<Box<dyn DownloadDelegate>>>,
}

impl WebView {
    /// Create a web view from a configuration.
    ///
    /// The configuration is consumed because script message handlers own
    /// their callbacks.
    pub fn new(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        let settings = wk::Settings::new();
        config.settings.apply_to(&settings);

        // The cache model lives on the shared web context. Without this the
        // engine stays at its `DocumentViewer` default, which keeps a tiny
        // memory cache and re-fetches/re-decodes images on every page.
        if let Some(context) = wk::WebContext::default() {
            context.set_cache_model(config.settings.engine_cache_model());
        }

        let mut builder = wk::WebView::builder();
        match &config.data_store {
            DataStoreKind::Default => {}
            DataStoreKind::Ephemeral => {
                let session = wk::NetworkSession::new_ephemeral();
                builder = builder.network_session(&session);
            }
            DataStoreKind::Custom {
                data_directory,
                cache_directory,
            } => {
                let session = wk::NetworkSession::builder()
                    .data_directory(data_directory.as_str())
                    .cache_directory(cache_directory.as_str())
                    .build();
                builder = builder.network_session(&session);
            }
        }
        builder = builder.settings(&settings);

        let ucm = wk::UserContentManager::new();
        for script in &config.user_scripts {
            ucm.add_script(&script.to_user_script());
        }
        for handler in config.message_handlers {
            register_message_handler(&ucm, handler);
        }
        builder = builder.user_content_manager(&ucm);

        let inner = builder.build();

        let view = Self {
            inner,
            delegate: Rc::new(RefCell::new(Box::new(DefaultWebViewDelegate))),
            navigation: Rc::new(RefCell::new(Box::new(DefaultWebNavigationDelegate))),
            downloads: Rc::new(RefCell::new(Box::new(DefaultDownloadDelegate))),
        };
        view.connect_signals();
        view.connect_downloads();

        if let Some(url) = config.start_url {
            view.load_url(&url)?;
        }

        Ok(view)
    }

    /// Fluent builder entry point.
    pub fn builder() -> WebViewBuilder {
        WebViewBuilder::new()
    }

    /// The underlying GTK4 widget. Use this to add the web view to a GTK
    /// container or a UIKit view.
    pub fn widget(&self) -> gtk::Widget {
        self.inner.clone().upcast()
    }

    /// Alias of [`WebView::widget`].
    pub fn to_gtk(&self) -> gtk::Widget {
        self.widget()
    }

    /// Access to the raw WebKitGTK web view.
    pub fn inner(&self) -> &wk::WebView {
        &self.inner
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

    /// The cookie manager of this view's network session, or `None` when
    /// the engine has no session attached yet.
    pub fn cookie_manager(&self) -> Option<crate::cookie::CookieManager> {
        crate::cookie::CookieManager::from_view(self)
    }

    /// Load a URL. Only `http(s)`, `file`, `data` and `about` URLs are
    /// accepted.
    pub fn load_url(&self, url: &str) -> Result<(), WebKitError> {
        validate_url(url)?;
        self.inner.load_uri(url);
        Ok(())
    }

    /// Load raw HTML content.
    pub fn load_html(&self, html: &str, base_uri: Option<&str>) {
        self.inner.load_html(html, base_uri);
    }

    /// Navigate back in the session history.
    pub fn go_back(&self) {
        self.inner.go_back();
    }

    /// Navigate forward in the session history.
    pub fn go_forward(&self) {
        self.inner.go_forward();
    }

    /// Whether there is a previous page in the history.
    pub fn can_go_back(&self) -> bool {
        self.inner.can_go_back()
    }

    /// Whether there is a next page in the history.
    pub fn can_go_forward(&self) -> bool {
        self.inner.can_go_forward()
    }

    /// Reload the current page.
    pub fn reload(&self) {
        self.inner.reload();
    }

    /// Reload the current page, bypassing caches.
    pub fn reload_bypass_cache(&self) {
        self.inner.reload_bypass_cache();
    }

    /// Stop the current load.
    pub fn stop_loading(&self) {
        self.inner.stop_loading();
    }

    /// The current page URL, if any.
    pub fn url(&self) -> Option<String> {
        self.inner.uri().map(|u| u.to_string())
    }

    /// The current page title, if any.
    pub fn title(&self) -> Option<String> {
        self.inner.title().map(|t| t.to_string())
    }

    /// Whether a page is currently loading.
    pub fn is_loading(&self) -> bool {
        self.inner.is_loading()
    }

    /// Estimated load progress between 0.0 and 1.0.
    pub fn estimated_progress(&self) -> f64 {
        self.inner.estimated_load_progress()
    }

    /// Current zoom level (1.0 = 100%).
    pub fn zoom_level(&self) -> f64 {
        self.inner.zoom_level()
    }

    /// Set the zoom level (1.0 = 100%).
    pub fn set_zoom_level(&self, level: f64) {
        self.inner.set_zoom_level(level);
    }

    /// Run JavaScript in the page and block for the JSON result.
    ///
    /// **This blocks the UI thread** until the engine returns the result,
    /// freezing rendering for the duration. Use it during startup or from
    /// the C FFI; prefer [`WebView::evaluate_javascript_async`] while the
    /// main loop is running.
    ///
    /// ```rust,no_run
    /// use webkit::{WebKitConfiguration, WebView};
    ///
    /// let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
    /// let title = web_view.evaluate_javascript("document.title").unwrap();
    /// ```
    pub fn evaluate_javascript(&self, script: &str) -> Result<serde_json::Value, WebKitError> {
        let future = self.inner.evaluate_javascript_future(script, None, None);
        let result = glib::MainContext::default()
            .block_on(future)
            .map_err(|e| WebKitError::Javascript(e.to_string()))?;
        Ok(crate::json::jsc_value_to_json(&result))
    }

    /// Run JavaScript in the page without blocking the UI thread.
    ///
    /// Returns a future that resolves when the engine has evaluated the
    /// script. Poll it on the GLib main context (for example with
    /// `glib::MainContext::default().spawn_local`) or await it inside an
    /// async context that drives the default main context.
    ///
    /// ```rust,no_run
    /// use webkit::{WebKitConfiguration, WebView};
    ///
    /// let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
    /// let future = web_view.evaluate_javascript_async("document.title").unwrap();
    /// glib::MainContext::default().spawn_local(async move {
    ///     if let Ok(title) = future.await {
    ///         println!("title: {title}");
    ///     }
    /// });
    /// ```
    pub fn evaluate_javascript_async(
        &self,
        script: &str,
    ) -> Result<impl std::future::Future<Output = Result<serde_json::Value, WebKitError>>, WebKitError>
    {
        let future = self.inner.evaluate_javascript_future(script, None, None);
        Ok(async move {
            let result = future.await.map_err(|e| WebKitError::Javascript(e.to_string()))?;
            Ok(crate::json::jsc_value_to_json(&result))
        })
    }

    /// Raw engine settings object (for settings not covered by
    /// [`WebSettings`]).
    pub fn engine_settings(&self) -> Option<wk::Settings> {
        webkit6::prelude::WebViewExt::settings(&self.inner)
    }

    /// Enable or disable the web inspector. Must be enabled (or set via
    /// `WebSettings::developer_extras`) before the inspector can open.
    pub fn set_developer_extras(&self, enabled: bool) {
        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&self.inner) {
            settings.set_enable_developer_extras(enabled);
        }
    }

    /// Whether the web inspector is currently attached to the view.
    pub fn is_inspector_open(&self) -> bool {
        self.inner
            .inspector()
            .map(|inspector| inspector.is_attached())
            .unwrap_or(false)
    }

    /// Show the web inspector.
    pub fn show_inspector(&self) {
        if let Some(inspector) = self.inner.inspector() {
            inspector.show();
        }
    }

    /// Close the web inspector.
    pub fn close_inspector(&self) {
        if let Some(inspector) = self.inner.inspector() {
            inspector.close();
        }
    }

    /// Toggle the web inspector; returns `true` when it is now open.
    pub fn toggle_inspector(&self) -> bool {
        if self.is_inspector_open() {
            self.close_inspector();
            false
        } else {
            self.show_inspector();
            true
        }
    }

    fn connect_signals(&self) {
        let nav = self.navigation.clone();
        let del = self.delegate.clone();
        self.inner.connect_load_changed(move |wv, event| {
            let url = wv.uri().map(|u| u.to_string());
            match load_event_to_navigation(&event) {
                NavigationEvent::Started => {
                    nav.borrow_mut().navigation_started(url.as_deref());
                    del.borrow_mut().load_started(url.as_deref());
                }
                NavigationEvent::Redirected => {
                    nav.borrow_mut().navigation_redirected(url.as_deref());
                }
                NavigationEvent::Committed => {
                    nav.borrow_mut().navigation_committed(url.as_deref());
                }
                NavigationEvent::Finished => {
                    nav.borrow_mut().navigation_finished(url.as_deref());
                    del.borrow_mut().load_finished(url.as_deref());
                }
                NavigationEvent::Failed => {}
            }
        });

        let nav = self.navigation.clone();
        let del = self.delegate.clone();
        self.inner.connect_load_failed(move |_wv, _event, uri, error| {
            let message = error.to_string();
            nav.borrow_mut().navigation_failed(Some(uri), &message);
            del.borrow_mut().load_failed(Some(uri), &message);
            false
        });

        let del = self.delegate.clone();
        self.inner.connect_title_notify(move |wv| {
            let title = wv.title().map(|t| t.to_string());
            del.borrow_mut().title_changed(title.as_deref());
        });

        let del = self.delegate.clone();
        self.inner.connect_uri_notify(move |wv| {
            let url = wv.uri().map(|u| u.to_string());
            del.borrow_mut().url_changed(url.as_deref());
        });

        let del = self.delegate.clone();
        self.inner.connect_estimated_load_progress_notify(move |wv| {
            del.borrow_mut().load_progress(wv.estimated_load_progress());
        });

        let del = self.delegate.clone();
        self.inner.connect_ready_to_show(move |_wv| {
            del.borrow_mut().ready_to_show();
        });

        let nav = self.navigation.clone();
        self.inner.connect_decide_policy(move |_wv, decision, decision_type| {
            let action = match decision_type {
                wk::PolicyDecisionType::NavigationAction => PolicyAction::Navigation,
                wk::PolicyDecisionType::NewWindowAction => PolicyAction::NewWindow,
                wk::PolicyDecisionType::Response => PolicyAction::Response,
                _ => PolicyAction::Response,
            };
            let url = if let Some(nav_decision) =
                decision.downcast_ref::<wk::NavigationPolicyDecision>()
            {
                nav_decision
                    .navigation_action()
                    .and_then(|mut nav_action| nav_action.request())
                    .and_then(|request| request.uri())
                    .map(|u| u.to_string())
            } else if let Some(res_decision) =
                decision.downcast_ref::<wk::ResponsePolicyDecision>()
            {
                res_decision
                    .request()
                    .and_then(|request| request.uri())
                    .map(|u| u.to_string())
            } else {
                None
            };
            let allow = nav.borrow_mut().decide_policy(url.as_deref(), action);
            if allow {
                decision.use_();
            } else {
                decision.ignore();
            }
            true
        });

        // window.open() loads into this same web view by default.
        let wv = self.inner.clone();
        self.inner.connect_create(move |_wv, _action| wv.clone().upcast());

        // JavaScript dialogs. The delegate answers; an unhandled dialog
        // gets the engine default (no UI, confirm = false, prompt = null).
        let del = self.delegate.clone();
        self.inner.connect_script_dialog(move |_wv, dialog| {
            let dialog = ScriptDialogRef::from_engine(dialog);
            del.borrow_mut().script_dialog(&dialog)
        });

        // Permission requests fail closed: the default delegate denies.
        let del = self.delegate.clone();
        self.inner.connect_permission_request(move |_wv, request| {
            let kind = permission_kind(request);
            match del.borrow_mut().permission_request(kind) {
                PermissionDecision::Grant => {
                    request.allow();
                    true
                }
                PermissionDecision::Deny => {
                    request.deny();
                    true
                }
            }
        });
    }

    /// Wire downloads of this view's network session to the download
    /// delegate.
    fn connect_downloads(&self) {
        let session = webkit6::prelude::WebViewExt::network_session(&self.inner);
        let Some(session) = session else {
            return;
        };
        let dl = self.downloads.clone();
        session.connect_download_started(move |_session, d| {
            let progress = dl.clone();
            d.connect_received_data(move |inner_d, _length| {
                let info = WebDownload::from_engine(inner_d.clone());
                progress.borrow_mut().download_progress(&info);
            });

            let failed = dl.clone();
            d.connect_failed(move |inner_d, error| {
                let info = WebDownload::from_engine(inner_d.clone());
                failed.borrow_mut().download_failed(&info, &error.to_string());
            });

            let finished = dl.clone();
            d.connect_finished(move |inner_d| {
                let info = WebDownload::from_engine(inner_d.clone());
                finished.borrow_mut().download_finished(&info);
            });

            let decide = dl.clone();
            d.connect_decide_destination(move |inner_d, suggested_filename| {
                let info = WebDownload::from_engine(inner_d.clone());
                let mut delegate = decide.borrow_mut();
                match delegate.decide_destination(&info, suggested_filename) {
                    Some(path) => {
                        inner_d.set_destination(&path);
                        true
                    }
                    None => false,
                }
            });
        });
    }
}

/// Map an engine permission request onto a [`PermissionKind`].
fn permission_kind(request: &wk::PermissionRequest) -> PermissionKind {
    if let Some(media) = request.downcast_ref::<wk::UserMediaPermissionRequest>() {
        return match (media.is_for_audio_device(), media.is_for_video_device()) {
            (true, true) => PermissionKind::CameraAndMicrophone,
            (true, false) => PermissionKind::Microphone,
            (false, true) => PermissionKind::Camera,
            (false, false) => PermissionKind::Other,
        };
    }
    if request.is::<wk::GeolocationPermissionRequest>() {
        return PermissionKind::Geolocation;
    }
    if request.is::<wk::NotificationPermissionRequest>() {
        return PermissionKind::Notifications;
    }
    if request.is::<wk::ClipboardPermissionRequest>() {
        return PermissionKind::ClipboardRead;
    }
    if request.is::<wk::DeviceInfoPermissionRequest>() {
        return PermissionKind::DeviceInfo;
    }
    if request.is::<wk::PointerLockPermissionRequest>() {
        return PermissionKind::PointerLock;
    }
    if request.is::<wk::MediaKeySystemPermissionRequest>() {
        return PermissionKind::MediaKeySystem;
    }
    if request.is::<wk::WebsiteDataAccessPermissionRequest>() {
        return PermissionKind::WebsiteDataAccess;
    }
    PermissionKind::Other
}

fn register_message_handler(ucm: &wk::UserContentManager, handler: ScriptMessageHandler) {    let name = handler.name.clone();
    if ucm.register_script_message_handler(&name, None) {
        let body = handler.body;
        ucm.connect_script_message_received(Some(&name), move |_ucm, value| {
            let json = crate::json::jsc_value_to_json(value);
            body(json);
        });
    }
}

/// Only these schemes may be loaded. Everything else (notably
/// `javascript:` and unknown custom schemes) is rejected.
const ALLOWED_URL_PREFIXES: [&str; 5] = ["http://", "https://", "file://", "data:", "about:"];

fn validate_url(url: &str) -> Result<(), WebKitError> {
    let lower = url.to_ascii_lowercase();
    if ALLOWED_URL_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        Ok(())
    } else {
        Err(WebKitError::InvalidUrl(url.to_string()))
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

    pub fn user_script(mut self, script: WebScript) -> Self {
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

    /// Build the web view. Fails only when the start URL is invalid.
    pub fn build(self) -> Result<WebView, WebKitError> {
        WebView::new(self.config)
    }
}

impl Default for WebViewBuilder {
    fn default() -> Self {
        Self::new()
    }
}