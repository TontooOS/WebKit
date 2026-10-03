//! WPE WebKit backend, rendered offscreen into the host window.
//!
//! Unlike Gecko this engine has no window of its own. WebKit renders into
//! a surfaceless EGL display we own, and every frame arrives as a SHM
//! buffer that the C shim copies into BGRA ([`crate::engine::SharedFrame`]).
//! The UI blits that frame as a Vello texture, so the page lives inside a
//! TontooUI window with our own toolbar, rounded corners and LiquidGlass.
//!
//! The path (see `src/wpe/tontoo_wpe.c` for the details):
//!
//! ```text
//! surfaceless EGL display          ours
//!   -> wpe_fdo_initialize_for_egl_display()
//!   -> wpe_view_backend_exportable_fdo_egl_create()
//!        export_shm_buffer -> BGRA copy -> SharedFrame
//!   -> webkit_web_view_backend_new() -> webkit_web_view_new()
//! ```
//!
//! No display server, no window, no GTK.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;

use foundation::serialization::JsonValue;

use crate::engine::{EngineCommand, EngineEvent, SharedFrame, WebEngine};

/// Engine event kinds, mirroring `tontoo_wpe.h`.
mod kind {
    pub const NONE: i32 = 0;
    pub const TITLE: i32 = 1;
    pub const URL: i32 = 2;
    pub const PROGRESS: i32 = 3;
    pub const LOAD_STARTED: i32 = 4;
    pub const LOAD_FINISHED: i32 = 5;
    pub const LOAD_FAILED: i32 = 6;
    pub const READY: i32 = 7;
    pub const JS_RESULT: i32 = 8;
    pub const COOKIES: i32 = 9;
    pub const COOKIE_DONE: i32 = 10;
    pub const SCRIPT_DIALOG: i32 = 11;
    pub const PERMISSION: i32 = 12;
    pub const HISTORY: i32 = 16;
}

/// Permission kinds, mirroring `tontoo_wpe.h`.
const PERM_OTHER: i64 = 0;
const PERM_GEOLOCATION: i64 = 1;
const PERM_NOTIFICATION: i64 = 2;
const PERM_USER_MEDIA: i64 = 3;

/// Pointer button kinds, mirroring `tontoo_wpe.h`.
const POINTER_MOVE: i32 = 0;
const POINTER_DOWN: i32 = 1;
const POINTER_UP: i32 = 2;

/// Keyboard modifiers understood by WPE.
const MOD_SHIFT: u32 = 1 << 1;
const MOD_CONTROL: u32 = 1 << 0;
const MOD_ALT: u32 = 1 << 2;

/// One event as the C shim reports it.
#[repr(C)]
struct RawEvent {
    kind: i32,
    id: i64,
    number: i64,
    text: [c_char; 1024],
}

impl RawEvent {
    fn text(&self) -> String {
        // SAFETY: the shim always NUL terminates `text`.
        unsafe { CStr::from_ptr(self.text.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

#[allow(non_camel_case_types)]
type TontooWpe = c_void;

// The shim exposes its full C surface; a few entry points (document-start
// injection, direct url/title/progress getters) are only used by embedders
// and tests, so unused ones must not warn.
#[allow(dead_code)]
extern "C" {
    fn tontoo_wpe_version() -> *const c_char;
    fn tontoo_wpe_backend_info() -> *const c_char;
    fn tontoo_wpe_new(
        width: u32,
        height: u32,
        url: *const c_char,
        err: *mut c_char,
        err_len: usize,
    ) -> *mut TontooWpe;
    fn tontoo_wpe_free(handle: *mut TontooWpe);
    fn tontoo_wpe_pump(handle: *mut TontooWpe, ms: i32);
    fn tontoo_wpe_next_event(handle: *mut TontooWpe, out: *mut RawEvent) -> c_int;
    fn tontoo_wpe_take_frame(
        handle: *mut TontooWpe,
        bgra: *mut *mut u8,
        w: *mut u32,
        h: *mut u32,
        seq: *mut u64,
    ) -> c_int;
    fn tontoo_wpe_free_buffer(buffer: *mut u8);
    fn tontoo_wpe_load(handle: *mut TontooWpe, url: *const c_char);
    fn tontoo_wpe_load_html(handle: *mut TontooWpe, html: *const c_char, base_uri: *const c_char);
    fn tontoo_wpe_reload(handle: *mut TontooWpe, bypass_cache: c_int);
    fn tontoo_wpe_go(handle: *mut TontooWpe, delta: c_int);
    fn tontoo_wpe_stop(handle: *mut TontooWpe);
    fn tontoo_wpe_resize(handle: *mut TontooWpe, width: u32, height: u32);
    fn tontoo_wpe_eval(handle: *mut TontooWpe, id: i64, script: *const c_char);
    fn tontoo_wpe_inject(handle: *mut TontooWpe, script: *const c_char);
    fn tontoo_wpe_pointer(
        handle: *mut TontooWpe,
        kind: c_int,
        x: f64,
        y: f64,
        button: u32,
    );
    fn tontoo_wpe_scroll(handle: *mut TontooWpe, x: f64, y: f64, dx: f64, dy: f64);
    fn tontoo_wpe_key(
        handle: *mut TontooWpe,
        key_code: u32,
        pressed: c_int,
        modifiers: u32,
        text: *const c_char,
    );
    fn tontoo_wpe_focus(handle: *mut TontooWpe, focused: c_int);
    fn tontoo_wpe_list_cookies(handle: *mut TontooWpe, id: i64);
    fn tontoo_wpe_set_cookie(
        handle: *mut TontooWpe,
        id: i64,
        name: *const c_char,
        value: *const c_char,
        domain: *const c_char,
        path: *const c_char,
        secure: c_int,
        http_only: c_int,
        expires: i64,
    );
    fn tontoo_wpe_delete_cookie(
        handle: *mut TontooWpe,
        id: i64,
        name: *const c_char,
        domain: *const c_char,
        path: *const c_char,
    );
    fn tontoo_wpe_clear_data(handle: *mut TontooWpe, cookies: c_int, cache: c_int);
    fn tontoo_wpe_answer_dialog(handle: *mut TontooWpe, id: i64, accept: c_int, text: *const c_char);
    fn tontoo_wpe_answer_permission(handle: *mut TontooWpe, id: i64, grant: c_int);
    fn tontoo_wpe_can_go_back(handle: *mut TontooWpe) -> c_int;
    fn tontoo_wpe_can_go_forward(handle: *mut TontooWpe) -> c_int;
    fn tontoo_wpe_url(handle: *mut TontooWpe) -> *const c_char;
    fn tontoo_wpe_title(handle: *mut TontooWpe) -> *const c_char;
    fn tontoo_wpe_progress(handle: *mut TontooWpe) -> f64;
}

/// Whether the C shim and its libraries are actually available.
pub fn is_available() -> bool {
    version().is_some()
}

/// Engine version string (`wpe-webkit 2.52.6, wpe 1.16.3`), or `None`
/// when WPE is not installed.
pub fn version() -> Option<String> {
    // SAFETY: the shim returns a static NUL terminated string or NULL.
    unsafe {
        let ptr = tontoo_wpe_version();
        if ptr.is_null() {
            return None;
        }
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

/// One line about the EGL backend in use, for logs and diagnostics.
pub fn backend_info() -> Option<String> {
    // SAFETY: same as `version`.
    unsafe {
        let ptr = tontoo_wpe_backend_info();
        if ptr.is_null() {
            return None;
        }
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

/// Launch options for [`WpeEngine`].
#[derive(Debug, Clone)]
pub struct WpeOptions {
    /// Initial frame buffer size in physical pixels.
    pub width: u32,
    /// Initial frame buffer height in physical pixels.
    pub height: u32,
    /// Page to open at startup.
    pub start_url: Option<String>,
    /// Ephemeral storage: cache and cookies live in memory only.
    pub private: bool,
    /// Where downloads are written.
    pub download_dir: PathBuf,
}

impl Default for WpeOptions {
    fn default() -> Self {
        Self {
            width: 1200,
            height: 800,
            start_url: None,
            private: false,
            download_dir: std::env::temp_dir().join("tontoo-webengine"),
        }
    }
}

/// DOM key codes for the keys a browser toolbar forwards.
fn dom_key_code(key: &str) -> Option<u32> {
    Some(match key {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Shift" => 16,
        "Control" | "Ctrl" => 17,
        "Alt" => 18,
        "Escape" => 27,
        " " | "Space" => 32,
        "PageUp" => 33,
        "PageDown" => 34,
        "End" => 35,
        "Home" => 36,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Delete" => 46,
        "F5" => 116,
        _ => return None,
    })
}

/// Modifier bits a key event carries. X11, Wayland and WPE all expect the
/// pressed modifier to be part of its own event.
fn modifier_bits(key: &str) -> u32 {
    match key {
        "Control" | "Ctrl" => MOD_CONTROL,
        "Alt" => MOD_ALT,
        "Shift" => MOD_SHIFT,
        _ => 0,
    }
}

/// Commands handed to the engine thread, which owns the WebKit view.
enum WpeCommand {
    Engine(EngineCommand),
    Focus(bool),
}

/// State that only the engine thread touches.
struct EngineState {
    url: Option<String>,
    width: u32,
    height: u32,
    /// Sequence number of the last frame handed to the UI.
    seq: u64,
    /// Last pointer position in CSS pixels, for wheel events.
    pointer: (f64, f64),
    /// Next correlation id for engine-side operations.
    next_id: u64,
}

/// Parse the JSON the shim produced for a cookie list.
fn parse_cookies(json: &str) -> Vec<crate::cookie::Cookie> {
    let Ok(value) = JsonValue::parse(json) else {
        return Vec::new();
    };
    let Some(list) = value.as_array() else {
        return Vec::new();
    };
    let text = |value: &JsonValue, key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let flag = |value: &JsonValue, key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    list.iter()
        .filter_map(|entry| {
            let name = text(entry, "name");
            if name.is_empty() {
                return None;
            }
            let mut cookie = crate::cookie::Cookie {
                name,
                value: text(entry, "value"),
                domain: text(entry, "domain"),
                path: text(entry, "path"),
                secure: flag(entry, "secure"),
                http_only: flag(entry, "httpOnly"),
                expires: entry.get("expires").and_then(|v| v.as_i64()),
            };
            if cookie.path.is_empty() {
                cookie.path = "/".to_string();
            }
            Some(cookie)
        })
        .collect()
}

/// Translate one raw C event into an engine event.
fn translate(raw: &RawEvent, state: &mut EngineState, out: &mut Vec<EngineEvent>) {
    let text = raw.text();
    match raw.kind {
        kind::TITLE => out.push(EngineEvent::Title(if text.is_empty() { None } else { Some(text) })),
        kind::URL => {
            state.url = if text.is_empty() { None } else { Some(text.clone()) };
            out.push(EngineEvent::Url(state.url.clone()));
        }
        kind::PROGRESS => out.push(EngineEvent::Progress(raw.number as f64 / 1_000_000.0)),
        kind::LOAD_STARTED => {
            state.url = Some(text.clone());
            out.push(EngineEvent::LoadStarted(Some(text)));
        }
        kind::LOAD_FINISHED => out.push(EngineEvent::LoadFinished(Some(text))),
        kind::LOAD_FAILED => out.push(EngineEvent::LoadFailed {
            url: state.url.clone(),
            error: text,
        }),
        kind::READY => out.push(EngineEvent::ReadyToShow),
        kind::JS_RESULT => {
            let value = JsonValue::parse(&text).unwrap_or(JsonValue::Null);
            out.push(EngineEvent::JsResult {
                id: raw.id as u64,
                result: value,
            });
        }
        kind::COOKIES => out.push(EngineEvent::Cookies {
            id: raw.id as u64,
            cookies: parse_cookies(&text),
        }),
        kind::COOKIE_DONE => {}
        kind::SCRIPT_DIALOG => {
            use crate::delegate::ScriptDialogKind;
            let dialog = match raw.number {
                1 => ScriptDialogKind::Confirm,
                2 => ScriptDialogKind::Prompt,
                3 => ScriptDialogKind::BeforeUnloadConfirm,
                _ => ScriptDialogKind::Alert,
            };
            out.push(EngineEvent::ScriptDialog {
                id: raw.id as u64,
                kind: dialog,
                message: text,
                prompt_default: None,
            });
        }
        kind::PERMISSION => {
            use crate::delegate::PermissionKind;
            let permission = match raw.number {
                PERM_OTHER => PermissionKind::Other,
                PERM_GEOLOCATION => PermissionKind::Geolocation,
                PERM_NOTIFICATION => PermissionKind::Notifications,
                PERM_USER_MEDIA => PermissionKind::MediaKeySystem,
                _ => PermissionKind::Other,
            };
            out.push(EngineEvent::PermissionRequest {
                id: raw.id as u64,
                kind: permission,
            });
        }
        kind::HISTORY => out.push(EngineEvent::History {
            can_back: raw.number & 1 != 0,
            can_forward: raw.number & 2 != 0,
        }),
        _ => {}
    }
}

/// Run one engine command against the view owned by the engine thread.
unsafe fn execute(
    handle: *mut TontooWpe,
    command: EngineCommand,
    state: &mut EngineState,
    out: &mut Vec<EngineEvent>,
) {
    let text = |value: &str| CString::new(value).unwrap_or_default();
    match command {
        EngineCommand::LoadUrl(url) => {
            state.url = Some(url.clone());
            out.push(EngineEvent::LoadStarted(Some(url.clone())));
            out.push(EngineEvent::Url(Some(url.clone())));
            tontoo_wpe_load(handle, text(&url).as_ptr());
        }
        EngineCommand::LoadHtml { html, base_uri } => {
            state.url = None;
            out.push(EngineEvent::LoadStarted(None));
            let base = base_uri.unwrap_or_else(|| "about:blank".to_string());
            tontoo_wpe_load_html(handle, text(&html).as_ptr(), text(&base).as_ptr());
        }
        EngineCommand::EvaluateJs { id, script } => {
            tontoo_wpe_eval(handle, id as i64, text(&script).as_ptr());
        }
        EngineCommand::Resize { width, height, .. } => {
            state.width = width;
            state.height = height;
            tontoo_wpe_resize(handle, width, height);
        }
        EngineCommand::MouseDown { x, y } => {
            state.pointer = (x, y);
            tontoo_wpe_pointer(handle, POINTER_MOVE, x, y, 0);
            tontoo_wpe_pointer(handle, POINTER_DOWN, x, y, 0);
        }
        EngineCommand::MouseUp { x, y } => {
            state.pointer = (x, y);
            tontoo_wpe_pointer(handle, POINTER_MOVE, x, y, 0);
            tontoo_wpe_pointer(handle, POINTER_UP, x, y, 0);
        }
        EngineCommand::MouseMove { x, y } => {
            state.pointer = (x, y);
            tontoo_wpe_pointer(handle, POINTER_MOVE, x, y, 0);
        }
        EngineCommand::Scroll { dx, dy } => {
            let (x, y) = state.pointer;
            tontoo_wpe_scroll(handle, x, y, dx, dy);
        }
        EngineCommand::KeyText(value) => {
            for ch in value.chars() {
                type_char(handle, ch);
            }
        }
        EngineCommand::SpecialKey { key } => {
            if let Some(code) = dom_key_code(&key) {
                let modifiers = modifier_bits(&key);
                tontoo_wpe_key(handle, code, 1, modifiers, std::ptr::null());
                tontoo_wpe_key(handle, code, 0, modifiers, std::ptr::null());
            } else {
                for ch in key.chars().take(1) {
                    type_char(handle, ch);
                }
            }
        }
        EngineCommand::Reload => tontoo_wpe_reload(handle, 0),
        EngineCommand::ReloadBypassCache => tontoo_wpe_reload(handle, 1),
        EngineCommand::GoBack => tontoo_wpe_go(handle, -1),
        EngineCommand::GoForward => tontoo_wpe_go(handle, 1),
        EngineCommand::Stop => tontoo_wpe_stop(handle),
        EngineCommand::ClearData { cookies, cache } => {
            tontoo_wpe_clear_data(handle, cookies as c_int, cache as c_int);
        }
        EngineCommand::ListCookies { id } => {
            tontoo_wpe_list_cookies(handle, id as i64);
        }
        EngineCommand::AddCookie { cookie } => {
            state.next_id += 1;
            tontoo_wpe_set_cookie(
                handle,
                state.next_id as i64,
                text(&cookie.name).as_ptr(),
                text(&cookie.value).as_ptr(),
                text(&cookie.domain).as_ptr(),
                text(&cookie.path).as_ptr(),
                cookie.secure as c_int,
                cookie.http_only as c_int,
                cookie.expires.unwrap_or(0),
            );
        }
        EngineCommand::DeleteCookie { domain, path, name } => {
            state.next_id += 1;
            tontoo_wpe_delete_cookie(
                handle,
                state.next_id as i64,
                text(&name).as_ptr(),
                text(&domain).as_ptr(),
                text(&path).as_ptr(),
            );
        }
        EngineCommand::DialogAnswer {
            id,
            confirmed,
            text: answer,
        } => {
            let value = answer.unwrap_or_default();
            tontoo_wpe_answer_dialog(handle, id as i64, confirmed as c_int, text(&value).as_ptr());
        }
        EngineCommand::PermissionAnswer { id, granted } => {
            tontoo_wpe_answer_permission(handle, id as i64, granted as c_int);
        }
        // Frames, script messages, downloads, tabs and extensions belong to
        // the other engines; WPE has no window and no add-on system.
        EngineCommand::DownloadDestination { .. }
        | EngineCommand::CancelDownload { .. }
        | EngineCommand::InstallExtension { .. }
        | EngineCommand::ListExtensions { .. }
        | EngineCommand::NewTab { .. }
        | EngineCommand::CloseTab { .. }
        | EngineCommand::ActivateTab { .. } => {}
    }
}

/// Type one character with a WPE key event pair.
unsafe fn type_char(handle: *mut TontooWpe, ch: char) {
    let upper = ch.to_ascii_uppercase();
    let shift = ch.is_ascii_uppercase() || ch.is_uppercase();
    let code = upper as u32;
    let modifiers = if shift { MOD_SHIFT } else { 0 };
    tontoo_wpe_key(handle, code, 1, modifiers, std::ptr::null());
    tontoo_wpe_key(handle, code, 0, modifiers, std::ptr::null());
}

/// State shared between the UI thread and the engine thread.
struct Shared {
    events: Mutex<Vec<EngineEvent>>,
    frame: Mutex<Option<Arc<SharedFrame>>>,
    running: AtomicBool,
    seq: AtomicU64,
}

/// [`WebEngine`] on top of an offscreen WPE WebKit view.
///
/// The engine owns one worker thread: it creates the WebKit view, pumps
/// the GLib main loop, executes commands and copies finished frames. The UI
/// thread only sends commands and reads the latest frame.
pub struct WpeEngine {
    shared: Arc<Shared>,
    sender: SyncSender<WpeCommand>,
}

impl WpeEngine {
    /// Create the engine and start the WebKit worker thread.
    pub fn launch(options: WpeOptions) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            events: Mutex::new(Vec::new()),
            frame: Mutex::new(None),
            running: AtomicBool::new(true),
            seq: AtomicU64::new(0),
        });
        let (sender, receiver) = sync_channel::<WpeCommand>(256);
        let (init_tx, init_rx) = sync_channel::<Result<(), String>>(1);
        let worker_shared = Arc::clone(&shared);

        std::thread::Builder::new()
            .name("tontoo-wpe".into())
            .spawn(move || worker(options, receiver, worker_shared, init_tx))
            .map_err(|e| format!("spawn wpe thread: {e}"))?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self { shared, sender }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("wpe engine thread died during startup".into()),
        }
    }

    /// Whether the worker thread is still alive.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }
}

/// The engine thread: create the view, then pump forever.
fn worker(
    options: WpeOptions,
    receiver: std::sync::mpsc::Receiver<WpeCommand>,
    shared: Arc<Shared>,
    init: SyncSender<Result<(), String>>,
) {
    let url = options.start_url.clone().unwrap_or_else(|| "about:blank".into());
    let url_c = CString::new(url.clone()).unwrap_or_default();
    let mut err = vec![0i8; 512];
    // SAFETY: the shim validates its arguments and reports failures in `err`.
    let handle = unsafe {
        tontoo_wpe_new(
            options.width.max(1),
            options.height.max(1),
            url_c.as_ptr(),
            err.as_mut_ptr(),
            err.len(),
        )
    };
    if handle.is_null() {
        // SAFETY: the shim NUL terminates the error buffer.
        let message = unsafe { CStr::from_ptr(err.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let _ = init.send(Err(if message.is_empty() {
            "WPE engine could not start".to_string()
        } else {
            message
        }));
        return;
    }
    let _ = init.send(Ok(()));

    let mut state = EngineState {
        url: Some(url),
        width: options.width.max(1),
        height: options.height.max(1),
        seq: 0,
        pointer: (0.0, 0.0),
        next_id: 1,
    };
    let mut events: Vec<EngineEvent> = Vec::new();
    let mut raw = RawEvent {
        kind: kind::NONE,
        id: 0,
        number: 0,
        text: [0; 1024],
    };

    while shared.running.load(Ordering::SeqCst) {
        loop {
            match receiver.try_recv() {
                Ok(WpeCommand::Engine(command)) => {
                    // SAFETY: `handle` is alive for the whole loop and all
                    // WebKit calls happen on this thread.
                    unsafe { execute(handle, command, &mut state, &mut events) };
                }
                Ok(WpeCommand::Focus(focused)) => {
                    // SAFETY: see above.
                    unsafe { tontoo_wpe_focus(handle, focused as c_int) };
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    shared.running.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }

        // SAFETY: see above; `raw` is a valid out-parameter.
        unsafe { tontoo_wpe_pump(handle, 8) };
        // SAFETY: see above.
        while unsafe { tontoo_wpe_next_event(handle, &mut raw) } != 0 {
            if raw.kind == kind::HISTORY {
                // The load-changed signal only knows the new document, so ask
                // the shim for the live back/forward state instead of trusting
                // the packed bits.
                // SAFETY: see above; the shim reads its own WebKit state.
                raw.number = i64::from(unsafe { tontoo_wpe_can_go_back(handle) } != 0)
                    | (i64::from(unsafe { tontoo_wpe_can_go_forward(handle) } != 0) << 1);
            }
            translate(&raw, &mut state, &mut events);
        }

        // Newest frame, copied once by the shim.
        let mut bgra: *mut u8 = std::ptr::null_mut();
        let mut width: u32 = 0;
        let mut height: u32 = 0;
        let mut seq: u64 = 0;
        // SAFETY: see above; the shim allocates exactly one buffer.
        if unsafe { tontoo_wpe_take_frame(handle, &mut bgra, &mut width, &mut height, &mut seq) }
            != 0
            && !bgra.is_null()
        {
            let len = width as usize * height as usize * 4;
            let slice = unsafe { std::slice::from_raw_parts(bgra, len) }.to_vec();
            unsafe { tontoo_wpe_free_buffer(bgra) };
            if seq != state.seq {
                state.seq = seq;
                shared.seq.store(seq, Ordering::SeqCst);
                *shared.frame.lock().expect("wpe frame lock") = Some(Arc::new(SharedFrame {
                    seq,
                    width,
                    height,
                    rgba: slice,
                }));
            }
        }

        if !events.is_empty() {
            shared
                .events
                .lock()
                .expect("wpe events lock")
                .append(&mut events);
        }
    }

    // SAFETY: `handle` was created on this thread and is freed exactly once.
    unsafe { tontoo_wpe_free(handle) };
}

impl WebEngine for WpeEngine {
    fn send(&self, command: EngineCommand) {
        let _ = self.sender.try_send(WpeCommand::Engine(command));
    }

    /// The newest rendered frame, BGRA, ready for the Vello texture path.
    fn latest_frame(&self) -> Option<Arc<SharedFrame>> {
        self.shared.frame.lock().expect("wpe frame lock").clone()
    }

    fn drain_events(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut *self.shared.events.lock().expect("wpe events lock"))
    }

    /// JavaScript answers inline on the worker thread and arrives as a
    /// `JsResult` event, so there is no synchronous path.
    fn eval_sync(&self, _script: &str) -> Option<JsonValue> {
        None
    }
}

impl Drop for WpeEngine {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
    }
}

/// Tell the engine whether the host window has keyboard focus.
pub fn set_focused(engine: &WpeEngine, focused: bool) {
    let _ = engine.sender.try_send(WpeCommand::Focus(focused));
}

/// Pump budget in milliseconds; the worker uses a fixed 8 ms slice.
#[cfg(test)]
pub(crate) fn millis(duration: Duration) -> i32 {
    duration.as_millis().min(i32::MAX as u128) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_key_codes_cover_the_toolbar_keys() {
        assert_eq!(dom_key_code("Enter"), Some(13));
        assert_eq!(dom_key_code("ArrowDown"), Some(40));
        assert_eq!(dom_key_code("Backspace"), Some(8));
        assert_eq!(dom_key_code("Escape"), Some(27));
        assert_eq!(dom_key_code("KeyA"), None);
    }

    #[test]
    fn modifier_keys_carry_their_own_bit() {
        assert_eq!(modifier_bits("Control"), MOD_CONTROL);
        assert_eq!(modifier_bits("Ctrl"), MOD_CONTROL);
        assert_eq!(modifier_bits("Alt"), MOD_ALT);
        assert_eq!(modifier_bits("Shift"), MOD_SHIFT);
        assert_eq!(modifier_bits("Enter"), 0);
        assert_eq!(dom_key_code("Control"), Some(17));
        assert_eq!(dom_key_code("Alt"), Some(18));
    }

    #[test]
    fn cookies_parse_from_the_shim_json() {
        let json = r#"[{"name":"a","value":"1","domain":"x.test","path":"/","secure":true,"httpOnly":false,"expires":42}]"#;
        let cookies = parse_cookies(json);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "a");
        assert!(cookies[0].secure);
        assert_eq!(cookies[0].expires, Some(42));
        assert!(parse_cookies("not json").is_empty());
        assert!(parse_cookies("[]").is_empty());
    }

    #[test]
    fn raw_events_translate() {
        let mut state = EngineState {
            url: None,
            width: 800,
            height: 600,
            seq: 0,
            pointer: (0.0, 0.0),
            next_id: 1,
        };
        let mut out = Vec::new();
        let mut raw = RawEvent {
            kind: kind::URL,
            id: 0,
            number: 0,
            text: [0; 1024],
        };
        for (i, ch) in "https://example.com".bytes().enumerate() {
            raw.text[i] = ch as c_char;
        }
        translate(&raw, &mut state, &mut out);
        match out.first() {
            Some(EngineEvent::Url(Some(url))) => assert_eq!(url, "https://example.com"),
            other => panic!("expected the page url, got {other:?}"),
        }

        out.clear();
        raw.kind = kind::PROGRESS;
        raw.number = 500_000;
        translate(&raw, &mut state, &mut out);
        match out.first() {
            Some(EngineEvent::Progress(value)) => assert_eq!(*value, 0.5),
            other => panic!("expected progress, got {other:?}"),
        }

        out.clear();
        raw.kind = kind::HISTORY;
        raw.number = 3; /* back | forward */
        translate(&raw, &mut state, &mut out);
        match out.first() {
            Some(EngineEvent::History {
                can_back,
                can_forward,
            }) => {
                assert!(can_back);
                assert!(can_forward);
            }
            other => panic!("expected history state, got {other:?}"),
        }

        out.clear();
        raw.kind = kind::JS_RESULT;
        raw.id = 7;
        raw.text = [0; 1024];
        for (i, ch) in b"{\"a\":1}".iter().enumerate() {
            raw.text[i] = *ch as c_char;
        }
        translate(&raw, &mut state, &mut out);
        match out.first() {
            Some(EngineEvent::JsResult { id, result }) => {
                assert_eq!(*id, 7);
                assert_eq!(result.get("a").and_then(|v| v.as_u64()), Some(1));
            }
            other => panic!("expected a JsResult, got {other:?}"),
        }
    }

    #[test]
    fn millis_clamps_to_c_int() {
        assert_eq!(millis(Duration::from_millis(8)), 8);
        assert_eq!(millis(Duration::from_secs(60)), 60_000);
    }
}
