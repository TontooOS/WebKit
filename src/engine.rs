//! Backend-neutral web engine abstraction.
//!
//! [`WebEngine`] decouples the public [`crate::WebView`] API from the HTML
//! engine underneath. The default build targets WPE WebKit running in the
//! out-of-process `tontoo-webengine` helper (same Apple WebKit engine as the
//! legacy GTK backend, but Wayland-native without GTK). Frames arrive as
//! BGRA pixel buffers and are blitted as Vello textures, so rounded corners
//! and LiquidGlass keep working.
//!
//! [`MockEngine`] renders a checkerboard without any system web engine. It
//! exists so the crate, its tests and TontooUI embedding compile and run
//! before the WPE helper lands.

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        body: serde_json::Value,
    },
    JsResult {
        id: u64,
        result: serde_json::Value,
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
    fn eval_sync(&self, _script: &str) -> Option<serde_json::Value> {
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

    fn eval_sync(&self, _script: &str) -> Option<serde_json::Value> {
        Some(serde_json::Value::Null)
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
