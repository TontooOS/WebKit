//! Backend-neutral web engine abstraction.
//!
//! [`WebEngine`] decouples the public [`crate::WebView`] API from the HTML
//! engine underneath. The default build targets WPE WebKit running in the
//! out-of-process `tontoo-webengine` helper (Apple WebKit, Wayland-native).
//! Frames arrive as
//! BGRA pixel buffers and are blitted as Vello textures, so rounded corners
//! and LiquidGlass keep working.
//!
//! [`MockEngine`] renders a checkerboard without any system web engine. It
//! exists so the crate, its tests and TontooUI embedding compile and run
//! before the WPE helper lands.

use std::sync::{Arc, RwLock};

use foundation::serialization::JsonValue;

use crate::cookie::Cookie;
use crate::delegate::{PermissionKind, ScriptDialogKind};

/// Only these schemes may be loaded. Everything else (notably
/// `javascript:` and unknown custom schemes) is rejected.
pub const ALLOWED_URL_PREFIXES: [&str; 5] =
    ["http://", "https://", "file://", "data:", "about:"];

/// Validate a URL against [`ALLOWED_URL_PREFIXES`].
pub fn validate_url(url: &str) -> Result<(), crate::WebKitError> {
    let lower = url.to_ascii_lowercase();
    if ALLOWED_URL_PREFIXES
        .iter()
        .any(|p| lower.starts_with(p))
    {
        Ok(())
    } else {
        Err(crate::WebKitError::InvalidUrl(url.to_string()))
    }
}

/// One rendered page frame: BGRA8 pixels shared with the UI thread.
#[derive(Debug, Clone)]
pub struct SharedFrame {
    /// Monotonic frame counter; the UI reuses its GPU upload while this
    /// matches (see `tontooui::ImageLoader::upload_frame`).
    pub seq: u64,
    /// Frame width in physical pixels.
    pub width: u32,
    /// Frame height in physical pixels.
    pub height: u32,
    /// Exactly `width * height * 4` BGRA bytes.
    pub rgba: Vec<u8>,
}

impl SharedFrame {
    /// Checkerboard placeholder used by [`MockEngine`] and while the real
    /// engine has not produced its first frame yet.
    pub fn checkerboard(seq: u64, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let i = (y as usize * width as usize + x as usize) * 4;
                // Dark-mode base (#1b2022) alternating with a lighter cell.
                let light = (x / 16 + y / 16) % 2 == 0;
                if light {
                    rgba[i] = 0x2c;
                    rgba[i + 1] = 0x34;
                    rgba[i + 2] = 0x36;
                } else {
                    rgba[i] = 0x1b;
                    rgba[i + 1] = 0x20;
                    rgba[i + 2] = 0x22;
                }
                rgba[i + 3] = 0xff;
            }
        }
        Self {
            seq,
            width,
            height,
            rgba,
        }
    }
}

/// Commands sent from the UI process to the engine process (or thread).
#[derive(Debug, Clone)]
pub enum EngineCommand {
    LoadUrl(String),
    LoadHtml {
        html: String,
        base_uri: Option<String>,
    },
    EvaluateJs {
        id: u64,
        script: String,
    },
    Resize {
        width: u32,
        height: u32,
        scale: f32,
    },
    MouseDown {
        x: f64,
        y: f64,
    },
    MouseUp {
        x: f64,
        y: f64,
    },
    MouseMove {
        x: f64,
        y: f64,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    KeyText(String),
    /// Non-printable key press ("Enter", "Backspace", "Escape",
    /// "ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown").
    SpecialKey {
        key: String,
    },
    Reload,
    ReloadBypassCache,
    GoBack,
    GoForward,
    Stop,
    /// Answer a script dialog (`ScriptDialog` event id).
    DialogAnswer {
        id: u64,
        confirmed: bool,
        text: Option<String>,
    },
    /// Answer a permission request (`PermissionRequest` event id).
    PermissionAnswer {
        id: u64,
        granted: bool,
    },
    /// Choose a download destination (`DownloadStarted` event id).
    /// `None` cancels the download.
    DownloadDestination {
        id: u64,
        path: Option<String>,
    },
    /// Cancel an in-progress download.
    CancelDownload {
        id: u64,
    },
    /// Clear stored website data in the engine.
    ClearData {
        cookies: bool,
        cache: bool,
    },
    /// List cookies of the engine store (answered by `Cookies`).
    ListCookies {
        id: u64,
    },
    /// Add or update a cookie in the engine store.
    AddCookie {
        cookie: Cookie,
    },
    /// Delete the cookie matching domain, path and name.
    DeleteCookie {
        domain: String,
        path: String,
        name: String,
    },
}

/// Events sent from the engine process back to the UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A new frame is ready in shared memory (carries seq + size only;
    /// pixels travel via shm, never over the socket).
    FrameReady {
        seq: u64,
        width: u32,
        height: u32,
    },
    Title(Option<String>),
    Url(Option<String>),
    Progress(f64),
    LoadStarted(Option<String>),
    LoadFinished(Option<String>),
    LoadFailed {
        url: Option<String>,
        error: String,
    },
    ScriptMessage {
        name: String,
        body: JsonValue,
    },
    JsResult {
        id: u64,
        result: JsonValue,
    },
    /// The page requested a JavaScript dialog. Answer with
    /// [`EngineCommand::DialogAnswer`].
    ScriptDialog {
        id: u64,
        kind: ScriptDialogKind,
        message: String,
        prompt_default: Option<String>,
    },
    /// The page requested a permission. Answer with
    /// [`EngineCommand::PermissionAnswer`].
    PermissionRequest {
        id: u64,
        kind: PermissionKind,
    },
    /// A download started. Answer with
    /// [`EngineCommand::DownloadDestination`].
    DownloadStarted {
        id: u64,
        uri: Option<String>,
        suggested_filename: String,
    },
    DownloadProgress {
        id: u64,
        progress: f64,
        received_bytes: u64,
    },
    DownloadFinished {
        id: u64,
    },
    DownloadFailed {
        id: u64,
        error: String,
    },
    /// Cookie list answer for [`EngineCommand::ListCookies`].
    Cookies {
        id: u64,
        cookies: Vec<Cookie>,
    },
    /// Session history state (back/forward availability).
    History {
        can_back: bool,
        can_forward: bool,
    },
    ReadyToShow,
}

/// The engine side of the web view. Implementations run the HTML engine
/// (WPE WebKit, Chromium CEF, Servo, or [`MockEngine`] for tests) and must
/// be safe to drive from the UI thread: every method only enqueues an
/// [`EngineCommand`] and returns immediately.
pub trait WebEngine: Send + Sync {
    /// Enqueue a command for the engine.
    fn send(&self, command: EngineCommand);

    /// The latest rendered frame, if any.
    fn latest_frame(&self) -> Option<Arc<SharedFrame>>;

    /// Load a URL after scheme validation.
    fn load_url(&self, url: &str) -> Result<(), crate::WebKitError> {
        validate_url(url)?;
        self.send(EngineCommand::LoadUrl(url.to_string()));
        Ok(())
    }

    /// Load raw HTML content.
    fn load_html(&self, html: &str, base_uri: Option<&str>) {
        self.send(EngineCommand::LoadHtml {
            html: html.to_string(),
            base_uri: base_uri.map(str::to_string),
        });
    }

    /// Notify the engine of its allocated size (physical pixels + scale).
    fn resize(&self, width: u32, height: u32, scale: f32) {
        self.send(EngineCommand::Resize {
            width: width.max(1),
            height: height.max(1),
            scale,
        });
    }

    /// Drain events queued by the engine since the last call.
    ///
    /// In-process engines return an empty vec; transports running a reader
    /// thread (out-of-process helper) return what arrived. The UI applies
    /// them with `WebView::pump_events`.
    fn drain_events(&self) -> Vec<EngineEvent> {
        Vec::new()
    }

    /// Synchronous JavaScript evaluation shortcut.
    ///
    /// Engines answering inline (tests) return `Some`; transports return
    /// `None` so the view correlates the async `JsResult` event instead.
    fn eval_sync(&self, _script: &str) -> Option<JsonValue> {
        None
    }
}

/// Test engine without any system web engine dependency.
///
/// Serves a checkerboard frame, records the last URL and answers every
/// JavaScript evaluation with `null`. Used by unit tests and as a
/// stand-in until the WPE helper process exists.
pub struct MockEngine {
    frame: RwLock<Arc<SharedFrame>>,
    url: RwLock<Option<String>>,
    seq: RwLock<u64>,
}

impl MockEngine {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            frame: RwLock::new(Arc::new(SharedFrame::checkerboard(0, width, height))),
            url: RwLock::new(None),
            seq: RwLock::new(0),
        }
    }

    /// Re-render the placeholder at a new size.
    pub fn resize_frame(&self, width: u32, height: u32) {
        let mut seq = self.seq.write().expect("mock seq lock");
        *seq += 1;
        *self.frame.write().expect("mock frame lock") =
            Arc::new(SharedFrame::checkerboard(*seq, width, height));
    }

    /// The last URL passed to [`WebEngine::load_url`].
    pub fn last_url(&self) -> Option<String> {
        self.url.read().expect("mock url lock").clone()
    }
}

impl WebEngine for MockEngine {
    fn send(&self, command: EngineCommand) {
        match command {
            EngineCommand::LoadUrl(url) => {
                *self.url.write().expect("mock url lock") = Some(url);
            }
            EngineCommand::Resize { width, height, .. } => {
                self.resize_frame(width, height);
            }
            _ => {}
        }
    }

    fn latest_frame(&self) -> Option<Arc<SharedFrame>> {
        Some(self.frame.read().expect("mock frame lock").clone())
    }

    fn eval_sync(&self, _script: &str) -> Option<JsonValue> {
        Some(JsonValue::Null)
    }
}

/// JSON wire encoding for the engine transport (replaces serde).
///
/// Shape matches the former serde externally-tagged format so recorded
/// lines stay readable: unit variants are bare strings (`"Reload"`),
/// other variants are single-key objects (`{"LoadFinished": ...}`).
mod protocol {
    use super::{Cookie, EngineCommand, EngineEvent, PermissionKind, ScriptDialogKind};
    use foundation::serialization::JsonValue;

    fn err_missing(field: &str) -> String {
        format!("missing field `{}`", field)
    }

    fn err_type(field: &str) -> String {
        format!("field `{}` has the wrong type", field)
    }

    fn req_str(doc: &JsonValue, key: &str) -> Result<String, String> {
        doc.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| err_missing(key))
    }

    fn opt_str(doc: &JsonValue, key: &str) -> Result<Option<String>, String> {
        match doc.get(key) {
            None | Some(JsonValue::Null) => Ok(None),
            Some(JsonValue::Str(s)) => Ok(Some(s.clone())),
            Some(_) => Err(err_type(key)),
        }
    }

    fn req_u64(doc: &JsonValue, key: &str) -> Result<u64, String> {
        doc.get(key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| err_missing(key))
    }

    fn req_u32(doc: &JsonValue, key: &str) -> Result<u32, String> {
        let v = req_u64(doc, key)?;
        u32::try_from(v).map_err(|_| format!("field `{}` out of range", key))
    }

    fn req_f64(doc: &JsonValue, key: &str) -> Result<f64, String> {
        doc.get(key)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| err_missing(key))
    }

    fn req_bool(doc: &JsonValue, key: &str) -> Result<bool, String> {
        doc.get(key)
            .and_then(|v| v.as_bool())
            .ok_or_else(|| err_missing(key))
    }

    fn str_field(key: &str, value: &str) -> (String, JsonValue) {
        (key.to_string(), JsonValue::Str(value.to_string()))
    }

    fn opt_str_field(key: &str, value: &Option<String>) -> (String, JsonValue) {
        let value = match value {
            Some(text) => JsonValue::Str(text.clone()),
            None => JsonValue::Null,
        };
        (key.to_string(), value)
    }

    fn dialog_kind_to_str(kind: ScriptDialogKind) -> &'static str {
        match kind {
            ScriptDialogKind::Alert => "Alert",
            ScriptDialogKind::Confirm => "Confirm",
            ScriptDialogKind::Prompt => "Prompt",
            ScriptDialogKind::BeforeUnloadConfirm => "BeforeUnloadConfirm",
        }
    }

    fn dialog_kind_from_str(s: &str) -> Option<ScriptDialogKind> {
        match s {
            "Alert" => Some(ScriptDialogKind::Alert),
            "Confirm" => Some(ScriptDialogKind::Confirm),
            "Prompt" => Some(ScriptDialogKind::Prompt),
            "BeforeUnloadConfirm" => Some(ScriptDialogKind::BeforeUnloadConfirm),
            _ => None,
        }
    }

    const PERMISSION_KINDS: &[(&str, PermissionKind)] = &[
        ("Camera", PermissionKind::Camera),
        ("Microphone", PermissionKind::Microphone),
        ("CameraAndMicrophone", PermissionKind::CameraAndMicrophone),
        ("Geolocation", PermissionKind::Geolocation),
        ("Notifications", PermissionKind::Notifications),
        ("ClipboardRead", PermissionKind::ClipboardRead),
        ("DeviceInfo", PermissionKind::DeviceInfo),
        ("PointerLock", PermissionKind::PointerLock),
        ("MediaKeySystem", PermissionKind::MediaKeySystem),
        ("WebsiteDataAccess", PermissionKind::WebsiteDataAccess),
        ("Other", PermissionKind::Other),
    ];

    fn permission_kind_to_str(kind: PermissionKind) -> &'static str {
        PERMISSION_KINDS
            .iter()
            .find(|(_, k)| *k == kind)
            .map(|(s, _)| *s)
            .unwrap_or("Other")
    }

    fn permission_kind_from_str(s: &str) -> Option<PermissionKind> {
        PERMISSION_KINDS
            .iter()
            .find(|(name, _)| *name == s)
            .map(|(_, k)| *k)
    }

    fn tagged(tag: &str, payload: JsonValue) -> JsonValue {
        JsonValue::Object(vec![(tag.to_string(), payload)])
    }

    impl EngineCommand {
        pub fn to_json_string(&self) -> String {
            self.to_json_value().to_compact_string()
        }

        fn to_json_value(&self) -> JsonValue {
            match self {
                Self::LoadUrl(url) => tagged("LoadUrl", JsonValue::Str(url.clone())),
                Self::LoadHtml { html, base_uri } => tagged(
                    "LoadHtml",
                    JsonValue::Object(vec![
                        str_field("html", html),
                        opt_str_field("base_uri", base_uri),
                    ]),
                ),
                Self::EvaluateJs { id, script } => tagged(
                    "EvaluateJs",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        str_field("script", script),
                    ]),
                ),
                Self::Resize { width, height, scale } => tagged(
                    "Resize",
                    JsonValue::Object(vec![
                        ("width".to_string(), JsonValue::Integer(*width as i64)),
                        ("height".to_string(), JsonValue::Integer(*height as i64)),
                        ("scale".to_string(), JsonValue::Float(*scale as f64)),
                    ]),
                ),
                Self::MouseDown { x, y } => tagged(
                    "MouseDown",
                    JsonValue::Object(vec![
                        ("x".to_string(), JsonValue::Float(*x)),
                        ("y".to_string(), JsonValue::Float(*y)),
                    ]),
                ),
                Self::MouseUp { x, y } => tagged(
                    "MouseUp",
                    JsonValue::Object(vec![
                        ("x".to_string(), JsonValue::Float(*x)),
                        ("y".to_string(), JsonValue::Float(*y)),
                    ]),
                ),
                Self::MouseMove { x, y } => tagged(
                    "MouseMove",
                    JsonValue::Object(vec![
                        ("x".to_string(), JsonValue::Float(*x)),
                        ("y".to_string(), JsonValue::Float(*y)),
                    ]),
                ),
                Self::Scroll { dx, dy } => tagged(
                    "Scroll",
                    JsonValue::Object(vec![
                        ("dx".to_string(), JsonValue::Float(*dx)),
                        ("dy".to_string(), JsonValue::Float(*dy)),
                    ]),
                ),
                Self::KeyText(text) => tagged("KeyText", JsonValue::Str(text.clone())),
                Self::SpecialKey { key } => tagged(
                    "SpecialKey",
                    JsonValue::Object(vec![str_field("key", key)]),
                ),
                Self::Reload => JsonValue::Str("Reload".to_string()),
                Self::ReloadBypassCache => JsonValue::Str("ReloadBypassCache".to_string()),
                Self::GoBack => JsonValue::Str("GoBack".to_string()),
                Self::GoForward => JsonValue::Str("GoForward".to_string()),
                Self::Stop => JsonValue::Str("Stop".to_string()),
                Self::DialogAnswer { id, confirmed, text } => tagged(
                    "DialogAnswer",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        ("confirmed".to_string(), JsonValue::Bool(*confirmed)),
                        opt_str_field("text", text),
                    ]),
                ),
                Self::PermissionAnswer { id, granted } => tagged(
                    "PermissionAnswer",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        ("granted".to_string(), JsonValue::Bool(*granted)),
                    ]),
                ),
                Self::DownloadDestination { id, path } => tagged(
                    "DownloadDestination",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        opt_str_field("path", path),
                    ]),
                ),
                Self::CancelDownload { id } => tagged(
                    "CancelDownload",
                    JsonValue::Object(vec![("id".to_string(), JsonValue::Integer(*id as i64))]),
                ),
                Self::ClearData { cookies, cache } => tagged(
                    "ClearData",
                    JsonValue::Object(vec![
                        ("cookies".to_string(), JsonValue::Bool(*cookies)),
                        ("cache".to_string(), JsonValue::Bool(*cache)),
                    ]),
                ),
                Self::ListCookies { id } => tagged(
                    "ListCookies",
                    JsonValue::Object(vec![("id".to_string(), JsonValue::Integer(*id as i64))]),
                ),
                Self::AddCookie { cookie } => tagged(
                    "AddCookie",
                    JsonValue::Object(vec![(
                        "cookie".to_string(),
                        cookie.to_json_value(),
                    )]),
                ),
                Self::DeleteCookie { domain, path, name } => tagged(
                    "DeleteCookie",
                    JsonValue::Object(vec![
                        str_field("domain", domain),
                        str_field("path", path),
                        str_field("name", name),
                    ]),
                ),
            }
        }

        pub fn from_json_str(s: &str) -> Result<Self, String> {
            let doc = JsonValue::parse(s).map_err(|e| e.to_string())?;
            Self::from_json_value(&doc)
        }

        fn from_json_value(doc: &JsonValue) -> Result<Self, String> {
            if let Some(tag) = doc.as_str() {
                return match tag {
                    "Reload" => Ok(Self::Reload),
                    "ReloadBypassCache" => Ok(Self::ReloadBypassCache),
                    "GoBack" => Ok(Self::GoBack),
                    "GoForward" => Ok(Self::GoForward),
                    "Stop" => Ok(Self::Stop),
                    other => Err(format!("unknown command `{}`", other)),
                };
            }
            let entries = doc
                .object_entries()
                .ok_or_else(|| "command must be an object".to_string())?;
            let (tag, payload) = entries
                .first()
                .ok_or_else(|| "empty command object".to_string())?;
            match tag.as_str() {
                "LoadUrl" => Ok(Self::LoadUrl(
                    payload
                        .as_str()
                        .ok_or_else(|| err_type("LoadUrl"))?
                        .to_string(),
                )),
                "LoadHtml" => Ok(Self::LoadHtml {
                    html: req_str(payload, "html")?,
                    base_uri: opt_str(payload, "base_uri")?,
                }),
                "EvaluateJs" => Ok(Self::EvaluateJs {
                    id: req_u64(payload, "id")?,
                    script: req_str(payload, "script")?,
                }),
                "Resize" => Ok(Self::Resize {
                    width: req_u32(payload, "width")?,
                    height: req_u32(payload, "height")?,
                    scale: req_f64(payload, "scale")? as f32,
                }),
                "MouseDown" => Ok(Self::MouseDown {
                    x: req_f64(payload, "x")?,
                    y: req_f64(payload, "y")?,
                }),
                "MouseUp" => Ok(Self::MouseUp {
                    x: req_f64(payload, "x")?,
                    y: req_f64(payload, "y")?,
                }),
                "MouseMove" => Ok(Self::MouseMove {
                    x: req_f64(payload, "x")?,
                    y: req_f64(payload, "y")?,
                }),
                "Scroll" => Ok(Self::Scroll {
                    dx: req_f64(payload, "dx")?,
                    dy: req_f64(payload, "dy")?,
                }),
                "KeyText" => Ok(Self::KeyText(
                    payload
                        .as_str()
                        .ok_or_else(|| err_type("KeyText"))?
                        .to_string(),
                )),
                "SpecialKey" => Ok(Self::SpecialKey {
                    key: req_str(payload, "key")?,
                }),
                "DialogAnswer" => Ok(Self::DialogAnswer {
                    id: req_u64(payload, "id")?,
                    confirmed: req_bool(payload, "confirmed")?,
                    text: opt_str(payload, "text")?,
                }),
                "PermissionAnswer" => Ok(Self::PermissionAnswer {
                    id: req_u64(payload, "id")?,
                    granted: req_bool(payload, "granted")?,
                }),
                "DownloadDestination" => Ok(Self::DownloadDestination {
                    id: req_u64(payload, "id")?,
                    path: opt_str(payload, "path")?,
                }),
                "CancelDownload" => Ok(Self::CancelDownload {
                    id: req_u64(payload, "id")?,
                }),
                "ClearData" => Ok(Self::ClearData {
                    cookies: req_bool(payload, "cookies")?,
                    cache: req_bool(payload, "cache")?,
                }),
                "ListCookies" => Ok(Self::ListCookies {
                    id: req_u64(payload, "id")?,
                }),
                "AddCookie" => Ok(Self::AddCookie {
                    cookie: Cookie::from_json_value(
                        payload
                            .get("cookie")
                            .ok_or_else(|| err_missing("cookie"))?,
                    ),
                }),
                "DeleteCookie" => Ok(Self::DeleteCookie {
                    domain: req_str(payload, "domain")?,
                    path: req_str(payload, "path")?,
                    name: req_str(payload, "name")?,
                }),
                other => Err(format!("unknown command `{}`", other)),
            }
        }
    }

    impl EngineEvent {
        pub fn to_json_string(&self) -> String {
            self.to_json_value().to_compact_string()
        }

        fn to_json_value(&self) -> JsonValue {
            match self {
                Self::FrameReady { seq, width, height } => tagged(
                    "FrameReady",
                    JsonValue::Object(vec![
                        ("seq".to_string(), JsonValue::Integer(*seq as i64)),
                        ("width".to_string(), JsonValue::Integer(*width as i64)),
                        ("height".to_string(), JsonValue::Integer(*height as i64)),
                    ]),
                ),
                Self::Title(value) => tagged("Title", opt_str_value(value)),
                Self::Url(value) => tagged("Url", opt_str_value(value)),
                Self::Progress(value) => tagged("Progress", JsonValue::Float(*value)),
                Self::LoadStarted(value) => tagged("LoadStarted", opt_str_value(value)),
                Self::LoadFinished(value) => tagged("LoadFinished", opt_str_value(value)),
                Self::LoadFailed { url, error } => tagged(
                    "LoadFailed",
                    JsonValue::Object(vec![
                        opt_str_field("url", url),
                        str_field("error", error),
                    ]),
                ),
                Self::ScriptMessage { name, body } => tagged(
                    "ScriptMessage",
                    JsonValue::Object(vec![
                        str_field("name", name),
                        ("body".to_string(), body.clone()),
                    ]),
                ),
                Self::JsResult { id, result } => tagged(
                    "JsResult",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        ("result".to_string(), result.clone()),
                    ]),
                ),
                Self::ScriptDialog { id, kind, message, prompt_default } => tagged(
                    "ScriptDialog",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        (
                            "kind".to_string(),
                            JsonValue::Str(dialog_kind_to_str(*kind).to_string()),
                        ),
                        str_field("message", message),
                        opt_str_field("prompt_default", prompt_default),
                    ]),
                ),
                Self::PermissionRequest { id, kind } => tagged(
                    "PermissionRequest",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        (
                            "kind".to_string(),
                            JsonValue::Str(permission_kind_to_str(*kind).to_string()),
                        ),
                    ]),
                ),
                Self::DownloadStarted { id, uri, suggested_filename } => tagged(
                    "DownloadStarted",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        opt_str_field("uri", uri),
                        str_field("suggested_filename", suggested_filename),
                    ]),
                ),
                Self::DownloadProgress { id, progress, received_bytes } => tagged(
                    "DownloadProgress",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        ("progress".to_string(), JsonValue::Float(*progress)),
                        (
                            "received_bytes".to_string(),
                            JsonValue::Integer(*received_bytes as i64),
                        ),
                    ]),
                ),
                Self::DownloadFinished { id } => tagged(
                    "DownloadFinished",
                    JsonValue::Object(vec![("id".to_string(), JsonValue::Integer(*id as i64))]),
                ),
                Self::DownloadFailed { id, error } => tagged(
                    "DownloadFailed",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        str_field("error", error),
                    ]),
                ),
                Self::Cookies { id, cookies } => tagged(
                    "Cookies",
                    JsonValue::Object(vec![
                        ("id".to_string(), JsonValue::Integer(*id as i64)),
                        (
                            "cookies".to_string(),
                            JsonValue::Array(
                                cookies.iter().map(|c| c.to_json_value()).collect(),
                            ),
                        ),
                    ]),
                ),
                Self::History { can_back, can_forward } => tagged(
                    "History",
                    JsonValue::Object(vec![
                        ("can_back".to_string(), JsonValue::Bool(*can_back)),
                        ("can_forward".to_string(), JsonValue::Bool(*can_forward)),
                    ]),
                ),
                Self::ReadyToShow => JsonValue::Str("ReadyToShow".to_string()),
            }
        }

        pub fn from_json_str(s: &str) -> Result<Self, String> {
            let doc = JsonValue::parse(s).map_err(|e| e.to_string())?;
            Self::from_json_value(&doc)
        }

        fn from_json_value(doc: &JsonValue) -> Result<Self, String> {
            if let Some(tag) = doc.as_str() {
                return match tag {
                    "ReadyToShow" => Ok(Self::ReadyToShow),
                    other => Err(format!("unknown event `{}`", other)),
                };
            }
            let entries = doc
                .object_entries()
                .ok_or_else(|| "event must be an object".to_string())?;
            let (tag, payload) = entries
                .first()
                .ok_or_else(|| "empty event object".to_string())?;
            match tag.as_str() {
                "FrameReady" => Ok(Self::FrameReady {
                    seq: req_u64(payload, "seq")?,
                    width: req_u32(payload, "width")?,
                    height: req_u32(payload, "height")?,
                }),
                "Title" => Ok(Self::Title(opt_payload_string(payload)?)),
                "Url" => Ok(Self::Url(opt_payload_string(payload)?)),
                "Progress" => Ok(Self::Progress(
                    payload.as_f64().ok_or_else(|| err_type("Progress"))?,
                )),
                "LoadStarted" => Ok(Self::LoadStarted(opt_payload_string(payload)?)),
                "LoadFinished" => Ok(Self::LoadFinished(opt_payload_string(payload)?)),
                "LoadFailed" => Ok(Self::LoadFailed {
                    url: opt_str(payload, "url")?,
                    error: req_str(payload, "error")?,
                }),
                "ScriptMessage" => Ok(Self::ScriptMessage {
                    name: req_str(payload, "name")?,
                    body: payload
                        .get("body")
                        .cloned()
                        .ok_or_else(|| err_missing("body"))?,
                }),
                "JsResult" => Ok(Self::JsResult {
                    id: req_u64(payload, "id")?,
                    result: payload
                        .get("result")
                        .cloned()
                        .ok_or_else(|| err_missing("result"))?,
                }),
                "ScriptDialog" => Ok(Self::ScriptDialog {
                    id: req_u64(payload, "id")?,
                    kind: payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .and_then(dialog_kind_from_str)
                        .ok_or_else(|| err_missing("kind"))?,
                    message: req_str(payload, "message")?,
                    prompt_default: opt_str(payload, "prompt_default")?,
                }),
                "PermissionRequest" => Ok(Self::PermissionRequest {
                    id: req_u64(payload, "id")?,
                    kind: payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .and_then(permission_kind_from_str)
                        .ok_or_else(|| err_missing("kind"))?,
                }),
                "DownloadStarted" => Ok(Self::DownloadStarted {
                    id: req_u64(payload, "id")?,
                    uri: opt_str(payload, "uri")?,
                    suggested_filename: req_str(payload, "suggested_filename")?,
                }),
                "DownloadProgress" => Ok(Self::DownloadProgress {
                    id: req_u64(payload, "id")?,
                    progress: req_f64(payload, "progress")?,
                    received_bytes: req_u64(payload, "received_bytes")?,
                }),
                "DownloadFinished" => Ok(Self::DownloadFinished {
                    id: req_u64(payload, "id")?,
                }),
                "DownloadFailed" => Ok(Self::DownloadFailed {
                    id: req_u64(payload, "id")?,
                    error: req_str(payload, "error")?,
                }),
                "Cookies" => {
                    let cookies = payload
                        .get("cookies")
                        .and_then(|v| v.as_array())
                        .ok_or_else(|| err_missing("cookies"))?;
                    Ok(Self::Cookies {
                        id: req_u64(payload, "id")?,
                        cookies: cookies.iter().map(Cookie::from_json_value).collect(),
                    })
                }
                "History" => Ok(Self::History {
                    can_back: req_bool(payload, "can_back")?,
                    can_forward: req_bool(payload, "can_forward")?,
                }),
                other => Err(format!("unknown event `{}`", other)),
            }
        }
    }

    fn opt_str_value(value: &Option<String>) -> JsonValue {
        match value {
            Some(text) => JsonValue::Str(text.clone()),
            None => JsonValue::Null,
        }
    }

    fn opt_payload_string(payload: &JsonValue) -> Result<Option<String>, String> {
        match payload {
            JsonValue::Null => Ok(None),
            JsonValue::Str(s) => Ok(Some(s.clone())),
            _ => Err(err_type("value")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_javascript_urls() {
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("HTTPS://example.com").is_ok());
    }

    #[test]
    fn mock_records_url_and_serves_frames() {
        let engine = MockEngine::new(320, 200);
        engine.load_url("https://example.com").unwrap();
        assert_eq!(engine.last_url().as_deref(), Some("https://example.com"));
        let frame = engine.latest_frame().unwrap();
        assert_eq!(frame.rgba.len(), 320 * 200 * 4);
        engine.resize(160, 100, 1.0);
        let frame = engine.latest_frame().unwrap();
        assert_eq!((frame.width, frame.height), (160, 100));
    }
}
