//! The backend-neutral [`WebView`], the equivalent of `WKWebView` in
//! Apple WebKit.
//!
//! This view owns no widget. It drives a [`crate::engine::WebEngine`]
//! (WPE WebKit out-of-process once `tontoo-webengine` lands,
//! [`crate::engine::MockEngine`] until then) and exposes the latest
//! [`crate::engine::SharedFrame`] for texture blitting into TontooUI
//! (see `crate::tontooui_view::WebViewContent`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

use crate::config::{DataStoreKind, WebKitConfiguration};
use crate::cookie::Cookie;
use crate::delegate::{
    DefaultWebViewDelegate, PermissionDecision, ScriptDialogRef, WebViewDelegate,
};
use crate::download::{DefaultDownloadDelegate, DownloadDelegate, WebDownload};
use crate::engine::{EngineCommand, MockEngine, SharedFrame, WebEngine};
use crate::error::WebKitError;
use crate::navigation::{DefaultWebNavigationDelegate, WebNavigationDelegate};
use crate::script::ScriptMessageHandler;
use crate::settings::WebSettings;
use foundation::serialization::JsonValue;

/// How long [`WebView::evaluate_javascript`] and [`WebView::list_cookies`]
/// wait for the engine answer while pumping events.
///
/// Blocking matches the legacy behavior (which blocked indefinitely);
/// slow first-starts (cold browser, software rendering) need the headroom.
/// Prefer a future async API for latency-sensitive UI code.
pub const JS_TIMEOUT: Duration = Duration::from_secs(60);

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
    next_js_id: Cell<u64>,
    pending_js: RefCell<HashMap<u64, SyncSender<JsonValue>>>,
    pending_cookies: RefCell<HashMap<u64, SyncSender<Vec<Cookie>>>>,
    pending_extension: RefCell<HashMap<u64, SyncSender<Result<String, String>>>>,
    pending_extensions: RefCell<HashMap<u64, SyncSender<Vec<String>>>>,
    engine_window: RefCell<Option<(u32, bool)>>,
    contexts: RefCell<Vec<String>>,
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
            next_js_id: Cell::new(1),
            pending_js: RefCell::new(HashMap::new()),
            pending_cookies: RefCell::new(HashMap::new()),
            pending_extension: RefCell::new(HashMap::new()),
            pending_extensions: RefCell::new(HashMap::new()),
            engine_window: RefCell::new(None),
            contexts: RefCell::new(Vec::new()),
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

    /// Create a web view, spawning the `tontoo-webengine` helper when it
    /// is available and falling back to the test engine otherwise.
    ///
    /// The helper is found via `find_helper`; `cargo run --example` does
    /// not build it, so run `cargo build` (or `cargo build --bin
    /// tontoo-webengine`) once first.
    pub fn with_spawned_engine(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        match crate::transport::ProcessEngine::spawn() {
            Ok(engine) => Self::with_engine(config, Arc::new(engine)),
            Err(e) => {
                eprintln!("tontoo-webengine unavailable ({e}); using test engine");
                Self::new(config)
            }
        }
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

    /// Forward a non-printable key press (`Enter`, `Backspace`, `Escape`,
    /// `ArrowLeft`, `ArrowUp`, `ArrowRight`, `ArrowDown`).
    pub fn press_key(&self, key: &str) {
        self.engine.send(EngineCommand::SpecialKey {
            key: key.to_string(),
        });
    }

    /// Apply one engine event (called by the IPC pump in the reader
    /// thread or the run loop; unit tests call it directly).
    ///
    /// Returns the number of applied events for [`WebView::pump_events`].
    /// Dialog, permission and download events are answered through the
    /// delegates immediately.
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
            E::FrameReady { .. } => {}
            E::JsResult { id, result } => {
                if let Some(tx) = self.pending_js.borrow_mut().remove(&id) {
                    let _ = tx.try_send(result);
                }
            }
            E::ScriptDialog {
                id,
                kind,
                message,
                prompt_default,
            } => {
                let dialog = ScriptDialogRef::new(kind, message, prompt_default);
                let handled = self.delegate.borrow_mut().script_dialog(&dialog);
                let (confirmed, text) = dialog.take_answer();
                self.engine.send(EngineCommand::DialogAnswer {
                    id,
                    confirmed: confirmed.unwrap_or(handled),
                    text,
                });
            }
            E::PermissionRequest { id, kind } => {
                let granted = matches!(
                    self.delegate.borrow_mut().permission_request(kind),
                    PermissionDecision::Grant
                );
                self.engine.send(EngineCommand::PermissionAnswer {
                    id,
                    granted,
                });
            }
            E::DownloadStarted {
                id,
                uri,
                suggested_filename,
            } => {
                let download = WebDownload {
                    uri,
                    destination: None,
                    progress: 0.0,
                    received_bytes: 0,
                };
                let path = self
                    .downloads
                    .borrow_mut()
                    .decide_destination(&download, &suggested_filename);
                self.engine.send(EngineCommand::DownloadDestination {
                    id,
                    path,
                });
            }
            E::DownloadProgress {
                id,
                progress,
                received_bytes,
            } => {
                let download = WebDownload {
                    uri: None,
                    destination: None,
                    progress,
                    received_bytes,
                };
                self.downloads
                    .borrow_mut()
                    .download_progress(&download);
                let _ = id;
            }
            E::DownloadFinished { id } => {
                let download = WebDownload {
                    uri: None,
                    destination: None,
                    progress: 1.0,
                    received_bytes: 0,
                };
                self.downloads.borrow_mut().download_finished(&download);
                let _ = id;
            }
            E::DownloadFailed { id, error } => {
                let download = WebDownload {
                    uri: None,
                    destination: None,
                    progress: 0.0,
                    received_bytes: 0,
                };
                self.downloads
                    .borrow_mut()
                    .download_failed(&download, &error);
                let _ = id;
            }
            E::Cookies { id, cookies } => {
                if let Some(tx) = self.pending_cookies.borrow_mut().remove(&id) {
                    let _ = tx.try_send(cookies);
                }
            }
            E::History {
                can_back,
                can_forward,
            } => {
                *self.can_back.borrow_mut() = can_back;
                *self.can_forward.borrow_mut() = can_forward;
            }
            E::EngineWindow { pid, kiosk } => {
                *self.engine_window.borrow_mut() = Some((pid, kiosk));
            }
            E::ContextReady { context } => {
                let mut contexts = self.contexts.borrow_mut();
                if !contexts.contains(&context) {
                    contexts.push(context);
                }
            }
            E::ContextClosed { context } => {
                self.contexts.borrow_mut().retain(|c| *c != context);
            }
            E::ExtensionInstalled { id, extension } => {
                if id == 0 {
                    self.delegate
                        .borrow_mut()
                        .extension_installed(&extension);
                    return;
                }
                if let Some(tx) = self.pending_extension.borrow_mut().remove(&id) {
                    let _ = tx.try_send(Ok(extension));
                }
            }
            E::ExtensionFailed { id, error } => {
                if let Some(tx) = self.pending_extension.borrow_mut().remove(&id) {
                    let _ = tx.try_send(Err(error));
                    return;
                }
                if let Some(tx) = self.pending_extensions.borrow_mut().remove(&id) {
                    let _ = tx.try_send(Vec::new());
                }
            }
            E::Extensions { id, extensions } => {
                if let Some(tx) = self.pending_extensions.borrow_mut().remove(&id) {
                    let _ = tx.try_send(extensions);
                }
            }
        }
    }

    /// Drain queued engine events and apply them. Call once per UI frame
    /// (before reading title/progress) when the engine runs
    /// out-of-process. Returns the number of applied events.
    pub fn pump_events(&self) -> usize {
        let events = self.engine.drain_events();
        let n = events.len();
        for event in events {
            self.apply_event(event);
        }
        n
    }

    /// Run JavaScript in the page and wait for the JSON result.
    ///
    /// Blocks the calling thread up to [`JS_TIMEOUT`] while pumping engine
    /// events. In-process test engines answer inline; the out-of-process
    /// helper answers through the correlated `JsResult` event. Returns
    /// `Err(WebKitError::Javascript)` on timeout.
    pub fn evaluate_javascript(
        &self,
        script: &str,
    ) -> Result<JsonValue, WebKitError> {
        if let Some(value) = self.engine.eval_sync(script) {
            return Ok(value);
        }
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        let (tx, rx) = sync_channel::<JsonValue>(1);
        self.pending_js.borrow_mut().insert(id, tx);
        self.engine.send(EngineCommand::EvaluateJs {
            id,
            script: script.to_string(),
        });
        let deadline = Instant::now() + JS_TIMEOUT;
        loop {
            self.pump_events();
            match rx.try_recv() {
                Ok(value) => return Ok(value),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending_js.borrow_mut().remove(&id);
                    return Err(WebKitError::Javascript("engine hung up".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending_js.borrow_mut().remove(&id);
                return Err(WebKitError::Javascript("timed out waiting for result".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// List cookies of the engine store. Blocks up to [`JS_TIMEOUT`].
    pub fn list_cookies(&self) -> Result<Vec<Cookie>, WebKitError> {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        let (tx, rx) = sync_channel::<Vec<Cookie>>(1);
        self.pending_cookies.borrow_mut().insert(id, tx);
        self.engine.send(EngineCommand::ListCookies { id });
        let deadline = Instant::now() + JS_TIMEOUT;
        loop {
            self.pump_events();
            match rx.try_recv() {
                Ok(cookies) => return Ok(cookies),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending_cookies.borrow_mut().remove(&id);
                    return Err(WebKitError::Engine("engine hung up".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending_cookies.borrow_mut().remove(&id);
                return Err(WebKitError::Engine("timed out waiting for cookies".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Add or update a cookie in the engine store.
    pub fn add_cookie(&self, cookie: &Cookie) {
        self.engine.send(EngineCommand::AddCookie {
            cookie: cookie.clone(),
        });
    }

    /// Delete the cookie matching domain, path and name.
    pub fn delete_cookie(&self, domain: &str, path: &str, name: &str) {
        self.engine.send(EngineCommand::DeleteCookie {
            domain: domain.to_string(),
            path: path.to_string(),
            name: name.to_string(),
        });
    }

    /// Clear stored website data in the engine.
    pub fn clear_data(&self, cookies: bool, cache: bool) {
        self.engine.send(EngineCommand::ClearData { cookies, cache });
    }

    /// Install a WebExtension from a local `manifest.json` directory or an
    /// `.xpi` archive and return its Firefox extension id.
    ///
    /// Blocks up to [`JS_TIMEOUT`] while pumping engine events. The
    /// extension is installed into the engine profile, so it survives
    /// restarts and updates itself from AMO. Unsigned packages need an ESR
    /// or DevEdition build: official release builds enforce Mozilla
    /// signatures regardless of the profile prefs.
    pub fn install_extension(&self, path: &std::path::Path) -> Result<String, WebKitError> {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        let (tx, rx) = sync_channel::<Result<String, String>>(1);
        self.pending_extension.borrow_mut().insert(id, tx);
        self.engine.send(EngineCommand::InstallExtension {
            id,
            path: path.to_string_lossy().to_string(),
        });
        let deadline = Instant::now() + JS_TIMEOUT;
        loop {
            self.pump_events();
            match rx.try_recv() {
                Ok(result) => {
                    return result.map_err(WebKitError::Engine);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending_extension.borrow_mut().remove(&id);
                    return Err(WebKitError::Engine("engine hung up".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending_extension.borrow_mut().remove(&id);
                return Err(WebKitError::Engine(
                    "timed out waiting for the extension".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// List the installed WebExtensions. Blocks up to [`JS_TIMEOUT`].
    ///
    /// Returns an empty vec when the engine cannot answer (mock engine).
    pub fn list_extensions(&self) -> Result<Vec<String>, WebKitError> {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        let (tx, rx) = sync_channel::<Vec<String>>(1);
        self.pending_extensions.borrow_mut().insert(id, tx);
        self.engine.send(EngineCommand::ListExtensions { id });
        let deadline = Instant::now() + JS_TIMEOUT;
        loop {
            self.pump_events();
            match rx.try_recv() {
                Ok(extensions) => return Ok(extensions),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending_extensions.borrow_mut().remove(&id);
                    return Err(WebKitError::Engine("engine hung up".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending_extensions.borrow_mut().remove(&id);
                return Err(WebKitError::Engine(
                    "timed out waiting for the extension list".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Open a new top-level context (`"tab"` or `"window"`).
    pub fn new_tab(&self, kind: &str) {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        self.engine.send(EngineCommand::NewTab {
            id,
            kind: kind.to_string(),
        });
    }

    /// Close a top-level context.
    pub fn close_tab(&self, context: &str) {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        self.engine.send(EngineCommand::CloseTab {
            id,
            context: context.to_string(),
        });
    }

    /// Focus a top-level context so its window comes forward.
    pub fn activate_tab(&self, context: &str) {
        let id = self.next_js_id.get();
        self.next_js_id.set(id.wrapping_add(1).max(1));
        self.engine.send(EngineCommand::ActivateTab {
            id,
            context: context.to_string(),
        });
    }

    /// Known top-level contexts (tabs and windows) the engine reported.
    pub fn contexts(&self) -> Vec<String> {
        self.contexts.borrow().clone()
    }

    /// The engine's own desktop window as `(pid, kiosk)`, once reported.
    ///
    /// Gecko draws into a real Wayland window instead of handing out
    /// frames. Look the pid up with `CoreWindows::list_windows` to
    /// position, minimize or close that window from a host app.
    pub fn engine_window(&self) -> Option<(u32, bool)> {
        *self.engine_window.borrow()
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
    use crate::engine::EngineEvent;
    use std::sync::Mutex;

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

    /// Test engine answering JS asynchronously through the event queue.
    struct PumpEngine {
        frame: Arc<SharedFrame>,
        pending: Mutex<Vec<EngineEvent>>,
    }

    impl WebEngine for PumpEngine {
        fn send(&self, command: EngineCommand) {
            if let EngineCommand::EvaluateJs { id, script } = command {
                let result = JsonValue::Str(format!("echo:{script}"));
                self.pending
                    .lock()
                    .unwrap()
                    .push(EngineEvent::JsResult { id, result });
            }
        }

        fn latest_frame(&self) -> Option<Arc<SharedFrame>> {
            Some(self.frame.clone())
        }

        fn drain_events(&self) -> Vec<EngineEvent> {
            std::mem::take(&mut *self.pending.lock().unwrap())
        }
    }

    #[test]
    fn js_result_correlation_works() {
        let engine = Arc::new(PumpEngine {
            frame: Arc::new(SharedFrame::checkerboard(0, 32, 32)),
            pending: Mutex::new(Vec::new()),
        });
        let view = WebView::with_engine(WebKitConfiguration::new(), engine).unwrap();
        let result = view.evaluate_javascript("1+1").unwrap();
        assert_eq!(result, JsonValue::Str("echo:1+1".into()));
    }

    #[test]
    fn dialog_answer_reaches_engine() {
        use crate::delegate::ScriptDialogKind;

        struct Deny;
        impl crate::delegate::WebViewDelegate for Deny {}

        let seen = Arc::new(Mutex::new(Vec::new()));
        struct Spy {
            seen: Arc<Mutex<Vec<EngineCommand>>>,
        }
        impl WebEngine for Spy {
            fn send(&self, command: EngineCommand) {
                // Only record answers, ignore the rest.
                if matches!(
                    command,
                    EngineCommand::DialogAnswer { .. } | EngineCommand::PermissionAnswer { .. }
                ) {
                    self.seen.lock().unwrap().push(command);
                }
            }
            fn latest_frame(&self) -> Option<Arc<SharedFrame>> {
                None
            }
        }
        let engine = Arc::new(Spy { seen: seen.clone() });
        let view = WebView::with_engine(WebKitConfiguration::new(), engine).unwrap();
        view.set_delegate(Box::new(Deny));
        view.apply_event(EngineEvent::ScriptDialog {
            id: 7,
            kind: ScriptDialogKind::Confirm,
            message: "sure?".into(),
            prompt_default: None,
        });
        view.apply_event(EngineEvent::PermissionRequest {
            id: 8,
            kind: crate::delegate::PermissionKind::Geolocation,
        });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(matches!(
            seen[0],
            EngineCommand::DialogAnswer { id: 7, .. }
        ));
        assert!(matches!(
            seen[1],
            EngineCommand::PermissionAnswer { id: 8, granted: false }
        ));
    }
}
