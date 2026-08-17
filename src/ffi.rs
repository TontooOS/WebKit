//! C FFI exposed from the `cdylib`. The matching header lives at
//! `Headers/webkit.h`.
//!
//! The FFI layer lets C / C++ apps (and other languages that can load a
//! shared library) embed a TontooWebKit web view. Configuration is passed
//! as a JSON string; state changes are reported through a callback vtable.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::rc::Rc;

use glib::translate::ToGlibPtr;
use serde::{Deserialize, Serialize};

use crate::config::{DataStoreKind, WebKitConfiguration};
use crate::delegate::WebViewDelegate;
use crate::error::WebKitError;
use crate::navigation::{PolicyAction, WebNavigationDelegate};
use crate::script::{ScriptMessageHandler, WebScript};
use crate::settings::WebSettings;
use crate::web_view::WebView;

/// Callback vtable used to report web view state to the C caller.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TontooWebViewCallbacks {
    /// The page title changed.
    pub on_title_changed:
        Option<unsafe extern "C" fn(user_data: *mut c_void, title: *const c_char)>,
    /// The current URL changed.
    pub on_url_changed: Option<unsafe extern "C" fn(user_data: *mut c_void, url: *const c_char)>,
    /// Load progress changed (0.0 ..= 1.0).
    pub on_load_progress: Option<unsafe extern "C" fn(user_data: *mut c_void, progress: c_double)>,
    /// The view started loading a page.
    pub on_load_started:
        Option<unsafe extern "C" fn(user_data: *mut c_void, url: *const c_char)>,
    /// The view finished loading a page.
    pub on_load_finished:
        Option<unsafe extern "C" fn(user_data: *mut c_void, url: *const c_char)>,
    /// Loading failed.
    pub on_load_failed:
        Option<unsafe extern "C" fn(user_data: *mut c_void, url: *const c_char, error: *const c_char)>,
    /// A page posted a message on a registered script channel.
    pub on_script_message: Option<
        unsafe extern "C" fn(user_data: *mut c_void, name: *const c_char, body_json: *const c_char),
    >,
    /// The web content is ready to be shown.
    pub on_ready_to_show: Option<unsafe extern "C" fn(user_data: *mut c_void)>,
    /// Decide whether a navigation is allowed. Returns nonzero to allow.
    pub on_decide_policy: Option<
        unsafe extern "C" fn(
            user_data: *mut c_void,
            url: *const c_char,
            action: c_int,
        ) -> c_int,
    >,
}

impl Default for TontooWebViewCallbacks {
    fn default() -> Self {
        Self {
            on_title_changed: None,
            on_url_changed: None,
            on_load_progress: None,
            on_load_started: None,
            on_load_finished: None,
            on_load_failed: None,
            on_script_message: None,
            on_ready_to_show: None,
            on_decide_policy: None,
        }
    }
}

/// Opaque web view handle handed to the C caller.
pub struct TontooWebView {
    view: WebView,
    shared: Rc<SharedFfi>,
}

/// Parsed FFI configuration.
#[derive(Debug, Default, Deserialize, Serialize)]
struct FfiConfig {
    #[serde(default)]
    start_url: Option<String>,
    #[serde(default)]
    settings: Option<WebSettings>,
    #[serde(default)]
    user_scripts: Vec<WebScript>,
    #[serde(default)]
    message_handlers: Vec<String>,
    #[serde(default)]
    private_browsing: bool,
    #[serde(default)]
    data_store: Option<FfiDataStore>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
struct FfiDataStore {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    data_directory: String,
    #[serde(default)]
    cache_directory: String,
}

/// Shared state between the view delegate, the navigation delegate and the
/// script message handlers.
struct SharedFfi {
    callbacks: RefCell<Option<TontooWebViewCallbacks>>,
    user_data: Cell<*mut c_void>,
}

impl SharedFfi {
    fn new() -> Self {
        Self {
            callbacks: RefCell::new(None),
            user_data: Cell::new(std::ptr::null_mut()),
        }
    }

    fn callbacks(&self) -> Option<TontooWebViewCallbacks> {
        self.callbacks.borrow().clone()
    }

    fn user_data(&self) -> *mut c_void {
        self.user_data.get()
    }
}

struct FfiDelegate {
    shared: Rc<SharedFfi>,
}

impl WebViewDelegate for FfiDelegate {
    fn title_changed(&mut self, title: Option<&str>) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_title_changed) {
            call_str(cb, self.shared.user_data(), title);
        }
    }

    fn url_changed(&mut self, url: Option<&str>) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_url_changed) {
            call_str(cb, self.shared.user_data(), url);
        }
    }

    fn load_progress(&mut self, progress: f64) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_load_progress) {
            unsafe {
                cb(self.shared.user_data(), progress);
            }
        }
    }

    fn load_started(&mut self, url: Option<&str>) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_load_started) {
            call_str(cb, self.shared.user_data(), url);
        }
    }

    fn load_finished(&mut self, url: Option<&str>) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_load_finished) {
            call_str(cb, self.shared.user_data(), url);
        }
    }

    fn load_failed(&mut self, url: Option<&str>, error: &str) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_load_failed) {
            call_str2(cb, self.shared.user_data(), url, Some(error));
        }
    }

    fn script_message(&mut self, _name: &str, _body: serde_json::Value) {}

    fn ready_to_show(&mut self) {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_ready_to_show) {
            unsafe {
                cb(self.shared.user_data());
            }
        }
    }
}

struct FfiNavigationDelegate {
    shared: Rc<SharedFfi>,
}

impl WebNavigationDelegate for FfiNavigationDelegate {
    fn decide_policy(&mut self, url: Option<&str>, action: PolicyAction) -> bool {
        let callbacks = self.shared.callbacks();
        if let Some(cb) = callbacks.and_then(|c| c.on_decide_policy) {
            let action_code = match action {
                PolicyAction::Navigation => 0,
                PolicyAction::NewWindow => 1,
                PolicyAction::Response => 2,
            };
            let url_c = url.and_then(|s| CString::new(s).ok());
            let url_ptr = url_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
            unsafe { cb(self.shared.user_data(), url_ptr, action_code) != 0 }
        } else {
            true
        }
    }
}

fn call_str(
    cb: unsafe extern "C" fn(*mut c_void, *const c_char),
    user_data: *mut c_void,
    value: Option<&str>,
) {
    let value_c = value.and_then(|s| CString::new(s).ok());
    let value_ptr = value_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    unsafe {
        cb(user_data, value_ptr);
    }
}

fn call_str2(
    cb: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
    user_data: *mut c_void,
    first: Option<&str>,
    second: Option<&str>,
) {
    let first_c = first.and_then(|s| CString::new(s).ok());
    let second_c = second.and_then(|s| CString::new(s).ok());
    let first_ptr = first_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    let second_ptr = second_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    unsafe {
        cb(user_data, first_ptr, second_ptr);
    }
}

unsafe fn cstring_ptr(s: &str) -> *mut c_char {
    CString::new(s)
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn set_error(out: *mut *mut c_char, message: &str) {
    if !out.is_null() {
        unsafe {
            *out = cstring_ptr(message);
        }
    }
}

fn parse_config(json: &str) -> Result<FfiConfig, WebKitError> {
    serde_json::from_str(json).map_err(|e| WebKitError::Engine(format!("invalid config: {e}")))
}

/// The framework version as a static C string.
#[no_mangle]
pub extern "C" fn tontoo_webkit_version() -> *const c_char {
    b"26.1.0\0".as_ptr() as *const c_char
}

/// Create a web view from a JSON configuration string.
///
/// Returns a new handle or `NULL`. On failure `error_out` receives a string
/// that must be freed with `tontoo_webkit_string_free`.
///
/// # Safety
///
/// `config_json` must be a valid NUL-terminated string. `error_out`, when
/// not null, must point to a `char*` that is either null or writable.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_new(
    config_json: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut TontooWebView {
    if config_json.is_null() {
        set_error(error_out, "config_json is null");
        return std::ptr::null_mut();
    }

    let json = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            set_error(error_out, "config_json is not valid UTF-8");
            return std::ptr::null_mut();
        }
    };

    let config = match parse_config(&json) {
        Ok(c) => c,
        Err(e) => {
            set_error(error_out, &e.to_string());
            return std::ptr::null_mut();
        }
    };

    let shared = Rc::new(SharedFfi::new());

    let message_handlers = config
        .message_handlers
        .iter()
        .map(|name| {
            let shared = shared.clone();
            let name = name.clone();
            ScriptMessageHandler::new(name.clone(), move |body: serde_json::Value| {
                let callbacks = shared.callbacks();
                if let Some(cb) = callbacks.and_then(|c| c.on_script_message) {
                    let name_c = CString::new(name.clone()).ok();
                    let body_c = CString::new(body.to_string()).ok();
                    let name_ptr = name_c
                        .as_ref()
                        .map_or(std::ptr::null(), |c| c.as_ptr());
                    let body_ptr = body_c
                        .as_ref()
                        .map_or(std::ptr::null(), |c| c.as_ptr());
                    unsafe {
                        cb(shared.user_data(), name_ptr, body_ptr);
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    let mut cfg = WebKitConfiguration::new();
    cfg.start_url = config.start_url.clone();
    cfg.settings = config.settings.clone().unwrap_or_default();
    cfg.user_scripts = config.user_scripts.clone();
    cfg.message_handlers = message_handlers;

    if let Some(ds) = &config.data_store {
        if !ds.kind.is_empty() {
            cfg.data_store = match ds.kind.as_str() {
                "ephemeral" | "private" => DataStoreKind::Ephemeral,
                "custom" => DataStoreKind::Custom {
                    data_directory: ds.data_directory.clone(),
                    cache_directory: ds.cache_directory.clone(),
                },
                _ => DataStoreKind::Default,
            };
        }
    }
    if config.private_browsing {
        cfg.data_store = DataStoreKind::Ephemeral;
    }

    let view = match WebView::new(cfg) {
        Ok(v) => v,
        Err(e) => {
            set_error(error_out, &e.to_string());
            return std::ptr::null_mut();
        }
    };

    view.set_delegate(Box::new(FfiDelegate { shared: shared.clone() }));
    view.set_navigation_delegate(Box::new(FfiNavigationDelegate { shared: shared.clone() }));

    Box::into_raw(Box::new(TontooWebView { view, shared }))
}

/// Install the callback vtable and user data for a web view.
///
/// # Safety
///
/// `view` must be a handle returned by `tontoo_webkit_view_new`. `user_data`
/// is passed back to every callback and must stay valid for as long as the
/// callbacks are installed.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_set_callbacks(
    view: *mut TontooWebView,
    callbacks: TontooWebViewCallbacks,
    user_data: *mut c_void,
) {
    let v = &*view;
    *v.shared.callbacks.borrow_mut() = Some(callbacks);
    v.shared.user_data.set(user_data);
}

/// The underlying GTK4 widget. Returned pointer is borrowed; do not free.
///
/// # Safety
///
/// `view` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_widget(
    view: *mut TontooWebView,
) -> *mut gtk::ffi::GtkWidget {
    let v = &*view;
    v.view.widget().to_glib_none().0
}

/// Load a URL. Returns 0 on success, -1 on error (see `error_out`).
///
/// # Safety
///
/// `view` must be valid; `url` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_load_url(
    view: *mut TontooWebView,
    url: *const c_char,
    error_out: *mut *mut c_char,
) -> c_int {
    let v = &*view;
    if url.is_null() {
        set_error(error_out, "url is null");
        return -1;
    }
    let url = match CStr::from_ptr(url).to_str() {
        Ok(u) => u,
        Err(_) => {
            set_error(error_out, "url is not valid UTF-8");
            return -1;
        }
    };
    match v.view.load_url(url) {
        Ok(()) => 0,
        Err(e) => {
            set_error(error_out, &e.to_string());
            -1
        }
    }
}

/// Load raw HTML content.
///
/// # Safety
///
/// `view` must be valid; `html` must be NUL-terminated. `base_uri` may be
/// null.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_load_html(
    view: *mut TontooWebView,
    html: *const c_char,
    base_uri: *const c_char,
) {
    let v = &*view;
    if html.is_null() {
        return;
    }
    let html = match CStr::from_ptr(html).to_str() {
        Ok(h) => h,
        Err(_) => return,
    };
    let base = if base_uri.is_null() {
        None
    } else {
        CStr::from_ptr(base_uri).to_str().ok()
    };
    v.view.load_html(html, base);
}

/// Navigate back in history.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_go_back(view: *mut TontooWebView) {
    (*view).view.go_back();
}

/// Navigate forward in history.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_go_forward(view: *mut TontooWebView) {
    (*view).view.go_forward();
}

/// Whether the view can go back. Returns nonzero when true.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_can_go_back(view: *mut TontooWebView) -> c_int {
    (*view).view.can_go_back() as c_int
}

/// Whether the view can go forward. Returns nonzero when true.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_can_go_forward(view: *mut TontooWebView) -> c_int {
    (*view).view.can_go_forward() as c_int
}

/// Reload the current page.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_reload(view: *mut TontooWebView) {
    (*view).view.reload();
}

/// Stop the current load.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_stop_loading(view: *mut TontooWebView) {
    (*view).view.stop_loading();
}

/// The current URL, or null. Free with `tontoo_webkit_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_get_url(view: *mut TontooWebView) -> *mut c_char {
    (*view)
        .view
        .url()
        .and_then(|u| CString::new(u).ok())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// The current page title, or null. Free with `tontoo_webkit_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_get_title(view: *mut TontooWebView) -> *mut c_char {
    (*view)
        .view
        .title()
        .and_then(|t| CString::new(t).ok())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Whether the view is currently loading. Returns nonzero when true.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_is_loading(view: *mut TontooWebView) -> c_int {
    (*view).view.is_loading() as c_int
}

/// Estimated load progress between 0.0 and 1.0.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_get_progress(view: *mut TontooWebView) -> c_double {
    (*view).view.estimated_progress()
}

/// Evaluate JavaScript and return the result as a JSON C string (or null on
/// error, see `error_out`). Free with `tontoo_webkit_string_free`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_evaluate_javascript(
    view: *mut TontooWebView,
    script: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let v = &*view;
    if script.is_null() {
        set_error(error_out, "script is null");
        return std::ptr::null_mut();
    }
    let script = match CStr::from_ptr(script).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_error(error_out, "script is not valid UTF-8");
            return std::ptr::null_mut();
        }
    };
    match v.view.evaluate_javascript(script) {
        Ok(json) => cstring_ptr(&json.to_string()),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Free a string returned by this library.
///
/// # Safety
///
/// `s` must be a pointer returned by `tontoo_webkit_view_get_url`,
/// `tontoo_webkit_view_get_title` or `tontoo_webkit_view_evaluate_javascript`,
/// or a `char*` written to `error_out`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// Destroy a web view handle.
///
/// # Safety
///
/// `view` must be a handle returned by `tontoo_webkit_view_new`. The handle
/// must not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn tontoo_webkit_view_free(view: *mut TontooWebView) {
    if !view.is_null() {
        drop(Box::from_raw(view));
    }
}