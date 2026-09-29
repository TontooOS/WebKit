//! C FFI for the Vello path (feature `vello`, no GTK).
//!
//! Polling model for non-Rust consumers: the caller drives the view,
//! copies BGRA frames with `tontoo_vello_view_poll_frame` and uploads them
//! as GPU textures. The matching header is `Headers/webkit_vello.h`.
//!
//! Strings returned by the library must be freed with
//! `tontoo_vello_string_free()`.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::sync::Mutex;

use foundation::serialization::JsonValue;

use crate::config::WebKitConfiguration;
use crate::delegate::WebViewDelegate;
use crate::error::WebKitError;
use crate::settings::WebSettings;
use crate::view::WebView;

/// Opaque Vello web view handle handed to the C caller.
pub struct TontooVelloView {
    view: WebView,
    last_seq: u64,
    messages: Mutex<Vec<(String, JsonValue)>>,
}

/// Parsed Vello FFI configuration.
#[derive(Debug, Default)]
struct VelloConfig {
    start_url: Option<String>,
    settings: Option<WebSettings>,
    private_browsing: bool,
    spawn_engine: bool,
}

impl VelloConfig {
    fn parse(json: &str) -> Result<Self, WebKitError> {
        let doc = JsonValue::parse(json)
            .map_err(|e| WebKitError::Engine(format!("invalid config: {e}")))?;
        Self::from_json_value(&doc)
    }

    fn from_json_value(doc: &JsonValue) -> Result<Self, WebKitError> {
        let settings = match doc.get("settings") {
            None | Some(JsonValue::Null) => None,
            Some(obj) => Some(WebSettings::from_json_value(obj).map_err(WebKitError::Engine)?),
        };
        Ok(Self {
            start_url: doc
                .get("start_url")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            settings,
            private_browsing: doc
                .get("private_browsing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            spawn_engine: doc
                .get("spawn_engine")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }
}

struct PollDelegate {
    messages: *const Mutex<Vec<(String, JsonValue)>>,
}

// SAFETY: the mutex outlives the view; only the UI thread calls into it.
unsafe impl Send for PollDelegate {}

impl WebViewDelegate for PollDelegate {
    fn script_message(&mut self, name: &str, body: JsonValue) {
        if let Some(queue) = unsafe { self.messages.as_ref() } {
            queue
                .lock()
                .expect("ffi messages lock")
                .push((name.to_string(), body));
        }
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

fn parse_config(json: &str) -> Result<VelloConfig, WebKitError> {
    VelloConfig::parse(json)
}

fn handle(view: *mut TontooVelloView) -> Option<&'static TontooVelloView> {
    if view.is_null() {
        None
    } else {
        unsafe { Some(&*view) }
    }
}

/// Framework version as a static C string.
#[no_mangle]
pub extern "C" fn tontoo_vello_version() -> *const c_char {
    b"27.0.0\0".as_ptr() as *const c_char
}

/// Create a Vello web view from a JSON configuration string.
///
/// When `"spawn_engine"` is true (or the config is empty) the helper is
/// spawned when available, otherwise the test engine is used. Returns a
/// new handle or `NULL` (see `error_out`).
///
/// # Safety
///
/// `config_json` must be a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_new(
    config_json: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut TontooVelloView {
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
    let mut cfg = WebKitConfiguration::new();
    cfg.start_url = config.start_url.clone();
    cfg.settings = config.settings.clone().unwrap_or_default();
    if config.private_browsing {
        cfg.data_store = crate::config::DataStoreKind::Ephemeral;
    }
    let view = if config.spawn_engine || json.trim() == "{}" || json.trim().is_empty() {
        WebView::with_spawned_engine(cfg)
    } else {
        WebView::new(cfg)
    };
    let view = match view {
        Ok(v) => v,
        Err(e) => {
            set_error(error_out, &e.to_string());
            return std::ptr::null_mut();
        }
    };
    let handle = Box::new(TontooVelloView {
        view,
        last_seq: u64::MAX,
        messages: Mutex::new(Vec::new()),
    });
    let messages_ptr: *const Mutex<Vec<(String, JsonValue)>> =
        std::ptr::from_ref(&handle.messages);
    handle.view.set_delegate(Box::new(PollDelegate {
        messages: messages_ptr,
    }));
    Box::into_raw(handle)
}

/// Destroy a Vello web view handle.
///
/// # Safety
///
/// `view` must be a handle returned by `tontoo_vello_view_new`.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_free(view: *mut TontooVelloView) {
    if !view.is_null() {
        drop(Box::from_raw(view));
    }
}

/// Apply queued engine events. Call once per UI frame before reading
/// state or polling frames.
///
/// # Safety
///
/// `view` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_pump(view: *mut TontooVelloView) -> c_int {
    match handle(view) {
        Some(h) => h.view.pump_events() as c_int,
        None => -1,
    }
}

/// Latest frame size. Returns 1 when a frame exists, 0 otherwise.
///
/// # Safety
///
/// `view` must be valid; `w`/`h` must be writable or NULL.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_frame_size(
    view: *mut TontooVelloView,
    w: *mut u32,
    ht: *mut u32,
) -> c_int {
    match handle(view).and_then(|h| h.view.poll_frame()) {
        Some(frame) => {
            if !w.is_null() {
                *w = frame.width;
            }
            if !ht.is_null() {
                *ht = frame.height;
            }
            1
        }
        None => 0,
    }
}

/// Copy the latest frame as BGRA bytes into `dst`.
///
/// Returns 1 when a new frame was copied, 0 when the frame is unchanged,
/// -1 on error (NULL handle, NULL buffer or buffer too small; see
/// `error_out`). `w`/`h` receive the frame size when non-NULL.
///
/// # Safety
///
/// `view` must be valid; `dst` must point to `dst_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_poll_frame(
    view: *mut TontooVelloView,
    dst: *mut c_void,
    dst_len: usize,
    w: *mut u32,
    h: *mut u32,
    error_out: *mut *mut c_char,
) -> c_int {
    let Some(handle) = handle(view) else {
        set_error(error_out, "view is null");
        return -1;
    };
    handle.view.pump_events();
    let Some(frame) = handle.view.poll_frame() else {
        return 0;
    };
    if frame.seq == unsafe { &mut *(view as *mut TontooVelloView) }.last_seq {
        return 0;
    }
    if dst.is_null() {
        set_error(error_out, "dst is null");
        return -1;
    }
    if dst_len < frame.rgba.len() {
        set_error(error_out, "dst buffer too small for frame");
        return -1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(frame.rgba.as_ptr(), dst as *mut u8, frame.rgba.len());
        if !w.is_null() {
            *w = frame.width;
        }
        if !h.is_null() {
            *h = frame.height;
        }
        (&mut *(view as *mut TontooVelloView)).last_seq = frame.seq;
    }
    1
}

/// Load a URL. Returns 0 on success, -1 on error.
///
/// # Safety
///
/// `view` must be valid; `url` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_load_url(
    view: *mut TontooVelloView,
    url: *const c_char,
    error_out: *mut *mut c_char,
) -> c_int {
    let Some(h) = handle(view) else {
        set_error(error_out, "view is null");
        return -1;
    };
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
    match h.view.load_url(url) {
        Ok(()) => 0,
        Err(e) => {
            set_error(error_out, &e.to_string());
            -1
        }
    }
}

/// Load raw HTML content. `base_uri` may be NULL.
///
/// # Safety
///
/// `view` must be valid; `html` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_load_html(
    view: *mut TontooVelloView,
    html: *const c_char,
    base_uri: *const c_char,
) {
    let (Some(h), false) = (handle(view), html.is_null()) else {
        return;
    };
    let Ok(html) = CStr::from_ptr(html).to_str() else {
        return;
    };
    let base = if base_uri.is_null() {
        None
    } else {
        CStr::from_ptr(base_uri).to_str().ok()
    };
    h.view.load_html(html, base);
}

/// Notify the engine of its allocated size (physical pixels + scale).
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_resize(
    view: *mut TontooVelloView,
    width: u32,
    height: u32,
    scale: f32,
) {
    if let Some(h) = handle(view) {
        h.view.resize(width, height, scale);
    }
}

/// Forward a mouse press in view coordinates (logical px).
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_mouse_down(
    view: *mut TontooVelloView,
    x: c_double,
    y: c_double,
) {
    if let Some(h) = handle(view) {
        h.view.mouse_down(x, y);
    }
}

/// Forward a mouse release.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_mouse_up(
    view: *mut TontooVelloView,
    x: c_double,
    y: c_double,
) {
    if let Some(h) = handle(view) {
        h.view.mouse_up(x, y);
    }
}

/// Forward a pointer move.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_mouse_move(
    view: *mut TontooVelloView,
    x: c_double,
    y: c_double,
) {
    if let Some(h) = handle(view) {
        h.view.mouse_move(x, y);
    }
}

/// Forward a scroll delta in logical px.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_scroll(
    view: *mut TontooVelloView,
    dx: c_double,
    dy: c_double,
) {
    if let Some(h) = handle(view) {
        h.view.scroll(dx, dy);
    }
}

/// Forward typed text (UTF-8, NUL-terminated).
///
/// # Safety
///
/// `view` must be valid; `text` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_key_text(
    view: *mut TontooVelloView,
    text: *const c_char,
) {
    if let (Some(h), Some(text)) = (
        handle(view),
        (!text.is_null())
            .then(|| CStr::from_ptr(text).to_str().ok())
            .flatten(),
    ) {
        h.view.input_text(text);
    }
}

/// Forward a non-printable key press (`Enter`, `Backspace`, `Escape`,
/// `ArrowLeft`, `ArrowUp`, `ArrowRight`, `ArrowDown`; NUL-terminated).
///
/// # Safety
///
/// `view` must be valid; `key` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_key_press(
    view: *mut TontooVelloView,
    key: *const c_char,
) {
    if let (Some(h), Some(key)) = (
        handle(view),
        (!key.is_null())
            .then(|| CStr::from_ptr(key).to_str().ok())
            .flatten(),
    ) {
        h.view.press_key(key);
    }
}

/// Evaluate JavaScript, blocking up to 5 seconds for the JSON result.
/// Returns NULL on error (see `error_out`). Free with
/// `tontoo_vello_string_free()`.
///
/// # Safety
///
/// `view` must be valid; `script` must be NUL-terminated.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_evaluate_javascript(
    view: *mut TontooVelloView,
    script: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(h) = handle(view) else {
        set_error(error_out, "view is null");
        return std::ptr::null_mut();
    };
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
    match h.view.evaluate_javascript(script) {
        Ok(json) => cstring_ptr(&json.to_compact_string()),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Take one queued script message. Returns 1 when a message was written
/// to `*name_out`/`*body_json_out` (both must be freed), 0 when the queue
/// is empty, -1 on a NULL handle.
///
/// # Safety
///
/// `view` must be valid; the out pointers must be writable.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_poll_script_message(
    view: *mut TontooVelloView,
    name_out: *mut *mut c_char,
    body_json_out: *mut *mut c_char,
) -> c_int {
    let Some(h) = handle(view) else {
        return -1;
    };
    h.view.pump_events();
    let mut queue = h.messages.lock().expect("ffi messages lock");
    if queue.is_empty() {
        return 0;
    }
    let (name, body) = queue.remove(0);
    if !name_out.is_null() {
        *name_out = cstring_ptr(&name);
    }
    if !body_json_out.is_null() {
        *body_json_out = cstring_ptr(&body.to_compact_string());
    }
    1
}

/// Current URL, or NULL. Free with `tontoo_vello_string_free()`.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_get_url(view: *mut TontooVelloView) -> *mut c_char {
    handle(view)
        .and_then(|h| h.view.url())
        .and_then(|u| CString::new(u).ok())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Current page title, or NULL. Free with `tontoo_vello_string_free()`.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_get_title(view: *mut TontooVelloView) -> *mut c_char {
    handle(view)
        .and_then(|h| h.view.title())
        .and_then(|t| CString::new(t).ok())
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Nonzero when the view is loading.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_is_loading(view: *mut TontooVelloView) -> c_int {
    handle(view).map(|h| h.view.is_loading() as c_int).unwrap_or(0)
}

/// Estimated load progress in [0.0, 1.0].
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_get_progress(view: *mut TontooVelloView) -> c_double {
    handle(view).map(|h| h.view.estimated_progress()).unwrap_or(0.0)
}

/// Navigate back / forward / reload / stop.
///
/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_go_back(view: *mut TontooVelloView) {
    if let Some(h) = handle(view) {
        h.view.go_back();
    }
}

/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_go_forward(view: *mut TontooVelloView) {
    if let Some(h) = handle(view) {
        h.view.go_forward();
    }
}

/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_reload(view: *mut TontooVelloView) {
    if let Some(h) = handle(view) {
        h.view.reload();
    }
}

/// # Safety
///
/// `view` must be valid.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_view_stop_loading(view: *mut TontooVelloView) {
    if let Some(h) = handle(view) {
        h.view.stop_loading();
    }
}

/// Free a string returned by this library.
///
/// # Safety
///
/// `s` must be a pointer returned by this library.
#[no_mangle]
pub unsafe extern "C" fn tontoo_vello_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
