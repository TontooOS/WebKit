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
    fn script_message(&mut self, _name: &str, _body: foundation::serialization::JsonValue) {}

    /// The web content is ready to be shown.
    fn ready_to_show(&mut self) {}

    /// A JavaScript dialog (`alert`, `confirm`, `prompt`) was requested.
    ///
    /// Return `true` when the app handled the dialog (it must then call
    /// [`ScriptDialogRef::set_confirmed`] or
    /// [`ScriptDialogRef::set_prompt_text`] as appropriate). Return
    /// `false` to leave it unhandled; the engine shows no UI and applies
    /// its default answer (`confirm` = false, `prompt` = null).
    fn script_dialog(&mut self, _dialog: &ScriptDialogRef) -> bool {
        false
    }

    /// The page requests a permission (camera, geolocation, ...).
    ///
    /// Defaults to [`PermissionDecision::Deny`] so nothing is ever granted
    /// silently.
    fn permission_request(&mut self, _kind: PermissionKind) -> PermissionDecision {
        PermissionDecision::Deny
    }

    /// A WebExtension was installed or updated by the engine itself
    /// (for example through its about:addons page). `id` is the Firefox
    /// extension id.
    ///
    /// Installations requested through
    /// [`crate::WebView::install_extension`] answer that call instead and
    /// are not reported here.
    fn extension_installed(&mut self, _id: &str) {}
}

/// Default delegate used when the caller does not provide one.
#[derive(Debug, Default)]
pub struct DefaultWebViewDelegate;

impl WebViewDelegate for DefaultWebViewDelegate {}

/// The kind of a JavaScript dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptDialogKind {
    /// `window.alert(message)`
    Alert,
    /// `window.confirm(message)` -- answer with `set_confirmed`.
    Confirm,
    /// `window.prompt(message, default)` -- answer with `set_prompt_text`.
    Prompt,
    /// An unload confirmation dialog.
    BeforeUnloadConfirm,
}

/// Backend-neutral script dialog passed to
/// [`WebViewDelegate::script_dialog`].
///
/// Carries the dialog kind, message and prompt default; answers are sent
/// back to the out-of-process engine by the view.
pub struct ScriptDialogRef {
    kind: ScriptDialogKind,
    message: String,
    prompt_default: Option<String>,
    confirmed: std::cell::Cell<Option<bool>>,
    prompt_text: std::cell::Cell<Option<String>>,
}

impl ScriptDialogRef {
    /// Create a dialog description (used by the engine IPC layer).
    pub fn new(
        kind: ScriptDialogKind,
        message: impl Into<String>,
        prompt_default: Option<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            prompt_default,
            confirmed: std::cell::Cell::new(None),
            prompt_text: std::cell::Cell::new(None),
        }
    }

    /// Which kind of dialog was requested.
    pub fn kind(&self) -> ScriptDialogKind {
        self.kind
    }

    /// The dialog message text.
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// The default text of a `prompt` dialog, if any.
    pub fn prompt_default_text(&self) -> Option<String> {
        self.prompt_default.clone()
    }

    /// Answer a `prompt` dialog with text.
    pub fn set_prompt_text(&self, text: &str) {
        self.prompt_text.set(Some(text.to_string()));
    }

    /// Answer a `confirm` or before-unload dialog.
    pub fn set_confirmed(&self, confirmed: bool) {
        self.confirmed.set(Some(confirmed));
    }

    /// Take the recorded answer: `(confirmed, prompt_text)`.
    pub fn take_answer(&self) -> (Option<bool>, Option<String>) {
        (self.confirmed.take(), self.prompt_text.take())
    }
}

/// What a page can ask permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// Camera access.
    Camera,
    /// Microphone access.
    Microphone,
    /// Camera and microphone at once.
    CameraAndMicrophone,
    /// Geolocation.
    Geolocation,
    /// Notifications.
    Notifications,
    /// Reading the system clipboard.
    ClipboardRead,
    /// Listing media devices.
    DeviceInfo,
    /// Locking the mouse pointer.
    PointerLock,
    /// Access to an EME media key system (DRM).
    MediaKeySystem,
    /// Cross-site website data access.
    WebsiteDataAccess,
    /// Any other or unknown permission.
    Other,
}

/// The app's decision for a [`WebViewDelegate::permission_request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Grant the permission.
    Grant,
    /// Deny the permission.
    Deny,
}
