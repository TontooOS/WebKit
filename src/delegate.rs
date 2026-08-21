//! View-level state callbacks, the equivalent of combining
//! `WKUIDelegate` and the `WKWebView` KVO notifications in Apple WebKit.

use webkit6 as wk;

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

/// Reference to an engine script dialog passed to
/// [`WebViewDelegate::script_dialog`].
pub struct ScriptDialogRef<'a> {
    inner: &'a wk::ScriptDialog,
}

impl<'a> ScriptDialogRef<'a> {
    pub(crate) fn from_engine(inner: &'a wk::ScriptDialog) -> Self {
        Self { inner }
    }

    /// Which kind of dialog was requested.
    pub fn kind(&self) -> ScriptDialogKind {
        match self.inner.dialog_type() {
            wk::ScriptDialogType::Confirm => ScriptDialogKind::Confirm,
            wk::ScriptDialogType::Prompt => ScriptDialogKind::Prompt,
            wk::ScriptDialogType::BeforeUnloadConfirm => ScriptDialogKind::BeforeUnloadConfirm,
            _ => ScriptDialogKind::Alert,
        }
    }

    /// The dialog message text.
    pub fn message(&self) -> String {
        self.inner.message().unwrap_or_default().to_string()
    }

    /// The default text of a `prompt` dialog, if any.
    pub fn prompt_default_text(&self) -> Option<String> {
        self.inner
            .prompt_get_default_text()
            .map(|t| t.to_string())
    }

    /// Answer a `prompt` dialog with text.
    pub fn set_prompt_text(&self, text: &str) {
        self.inner.prompt_set_text(text);
    }

    /// Answer a `confirm` or before-unload dialog.
    pub fn set_confirmed(&self, confirmed: bool) {
        self.inner.confirm_set_confirmed(confirmed);
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
