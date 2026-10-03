//! Gecko engine backend: headful Firefox driven over WebDriver BiDi.
//!
//! Firefox owns its own Wayland window and renders at full frame rate, so
//! this backend is not a frame producer: pages, WebExtensions, WebGL, video
//! and printing all run inside Gecko while the host process only sends
//! commands and reads JSON state. That makes it the engine that can host a
//! full browser (uBlock Origin, Vimium, Bitwarden, ...) on TontooOS.
//!
//! ## Lifecycle
//!
//! 1. [`find_firefox`] locates a binary (`TONTOO_FIREFOX_BIN`, the managed
//!    ESR build from [`crate::gecko_provision`], then `PATH` names).
//! 2. [`GeckoPage::launch`] writes a private profile (`user.js` with the
//!    remote agent, download dir and privacy prefs), spawns Firefox with
//!    `--remote-debugging-port`, and opens a BiDi WebSocket on
//!    `ws://127.0.0.1:<port>/session`.
//! 3. `session.new` + `session.subscribe`, then the top-level browsing
//!    context is taken from `browsingContext.getTree`.
//! 4. Every [`BidiEvent`] lands on the receiver handed out by `launch`;
//!    the helper turns those into [`crate::EngineEvent`] values.
//!
//! ## Security
//!
//! The remote agent listens on loopback only and Firefox keeps its own
//! process sandbox, content process isolation and site isolation. Host
//! processes never see page pixels or the DOM, only JSON.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use foundation::serialization::JsonValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

/// How long a single BiDi round-trip may take.
pub const BIDI_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for the remote agent WebSocket at launch. A cold
/// Firefox with a fresh profile also pays for the fontconfig scan.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Budget for one `session.subscribe`. Firefox sometimes never answers a
/// multi-event subscribe, so this stays short.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(4);

/// Subscribed BiDi events. Everything the engine protocol needs is
/// requested up front; names a given Firefox build does not know are
/// skipped by [`subscribe_events`], so this list is safe across releases.
pub const SUBSCRIBED_EVENTS: [&str; 13] = [
    "browsingContext.contextCreated",
    "browsingContext.contextDestroyed",
    "browsingContext.navigationStarted",
    "browsingContext.domContentLoaded",
    "browsingContext.load",
    "browsingContext.userPromptOpened",
    // Downloads moved from the browsingContext to the network module;
    // both spellings are requested and whichever the build knows wins.
    "browsingContext.downloadWillBegin",
    "browsingContext.downloadProgress",
    "network.downloadWillBegin",
    "network.downloadProgress",
    "log.entryAdded",
    "script.message",
    "webExtension.installed",
];

/// WebDriver-BiDi key values for the WebDriver PUA code points.
pub fn bidi_key_value(key: &str) -> Option<&'static str> {
    Some(match key {
        "Enter" => "\u{E007}",
        "Tab" => "\u{E004}",
        "Backspace" => "\u{E003}",
        "Delete" => "\u{E017}",
        "Escape" => "\u{E00C}",
        "Space" => "\u{E00D}",
        "ArrowLeft" => "\u{E012}",
        "ArrowUp" => "\u{E013}",
        "ArrowRight" => "\u{E014}",
        "ArrowDown" => "\u{E015}",
        "Home" => "\u{E011}",
        "End" => "\u{E010}",
        "PageUp" => "\u{E00E}",
        "PageDown" => "\u{E00F}",
        "Insert" => "\u{E016}",
        "Shift" => "\u{E008}",
        "Control" => "\u{E009}",
        "Alt" => "\u{E00A}",
        _ => return None,
    })
}

/// Locate a Firefox binary: `TONTOO_FIREFOX_BIN`/`FIREFOX_BIN`, the
/// managed self-provisioned build, then well-known names on `PATH`
/// (smoke-tested, so a broken system build is skipped).
///
/// Never touches the network. Use [`crate::ensure_firefox`] when a missing
/// browser should be provisioned (an app that wants a real engine), and
/// let [`crate::gecko_provision::maybe_background_update`] download it
/// for the next start.
pub fn find_firefox() -> Result<PathBuf, String> {
    for var in ["TONTOO_FIREFOX_BIN", "FIREFOX_BIN"] {
        if let Ok(path) = std::env::var(var) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    if let Some((_, bin)) = super::gecko_provision::installed() {
        return Ok(bin);
    }
    for name in [
        "firefox",
        "firefox-esr",
        "firefox-devedition",
        "firefox-bin",
        "firefox.exe",
    ] {
        if let Some(path) = super::transport::find_on_path(name) {
            if super::gecko_provision::smoke_test(&path) {
                return Ok(path);
            }
            eprintln!("tontoo-webengine: system {name} does not run, skipping");
        }
    }
    Err("no Firefox binary found".into())
}

/// Private profile directory for a session.
///
/// Persistent sessions live under the managed dir so cookies, logins and
/// installed extensions survive restarts; private sessions get a throwaway
/// directory that is deleted when Firefox exits.
pub fn profile_dir(private: bool) -> Result<PathBuf, String> {
    if private {
        let dir = std::env::temp_dir().join(format!(
            "tontoo-gecko-private-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("profile: {e}"))?;
        return Ok(dir);
    }
    let dir = super::gecko_provision::managed_dir().join("profile");
    std::fs::create_dir_all(&dir).map_err(|e| format!("profile: {e}"))?;
    Ok(dir)
}

/// Prefs written into every managed profile.
///
/// `remote.active-protocols = 2` enables WebDriver BiDi only (1 would be the
/// CDP compatibility layer, 3 both). Firefox' own updater is off because
/// [`crate::gecko_provision`] owns updates; the extension updater stays on
/// so AMO-hosted add-ons keep themselves current.
fn profile_prefs(port: u16, download_dir: &Path, private: bool) -> Vec<(String, String)> {
    let mut prefs: Vec<(String, String)> = vec![
        // BiDi (2) plus the DevTools layer (1). Firefox only registers
        // the `/session` BiDi handler when both are active; with 2 the
        // endpoint answers 404 "Unknown command".
        ("remote.active-protocols".into(), "3".into()),
        ("remote.enabled".into(), "true".into()),
        ("remote.allow-hosts".into(), "localhost,127.0.0.1".into()),
        ("devtools.console.stdout.content".into(), "true".into()),
        ("browser.shell.checkDefaultBrowser".into(), "false".into()),
        ("browser.startup.homepage".into(), "about:blank".into()),
        ("browser.startup.page".into(), "0".into()),
        ("browser.startup.homepage_override.mstone".into(), "ignore".into()),
        ("browser.aboutwelcome.enabled".into(), "false".into()),
        ("browser.sessionstore.resume_from_crash".into(), "false".into()),
        ("browser.tabs.warnOnClose".into(), "false".into()),
        ("browser.warnOnQuitShortcut".into(), "false".into()),
        ("browser.download.folderList".into(), "2".into()),
        (
            "browser.download.dir".into(),
            download_dir.to_string_lossy().to_string(),
        ),
        ("browser.download.useDownloadDir".into(), "true".into()),
        ("browser.download.alwaysOpenPanel".into(), "false".into()),
        ("browser.download.manager.showWhenStarting".into(), "false".into()),
        ("browser.link.open_newwindow".into(), "3".into()),
        ("browser.tabs.remote.autostart".into(), "true".into()),
        // Firefox manages the OS window, so hide its own chrome: the host
        // draws the TontooUI toolbar.
        ("browser.tabs.inTitlebar".into(), "0".into()),
        ("browser.uidensity".into(), "1".into()),
        // Managed by gecko_provision, not by Firefox itself.
        ("app.update.auto".into(), "false".into()),
        ("app.update.enabled".into(), "false".into()),
        ("app.update.service.enabled".into(), "false".into()),
        // Privacy: no telemetry, no studies, no background pings.
        ("datareporting.policy.dataSubmissionEnabled".into(), "false".into()),
        ("datareporting.healthreport.uploadEnabled".into(), "false".into()),
        ("toolkit.telemetry.enabled".into(), "false".into()),
        ("toolkit.telemetry.unified".into(), "false".into()),
        ("toolkit.telemetry.server".into(), String::new()),
        ("app.shield.optoutstudies.enabled".into(), "false".into()),
        ("browser.ping-centre.telemetry".into(), "false".into()),
        // WebExtensions: keep them enabled for every scope (policy,
        // sideloaded, temporary) and allow unsigned local packages. The
        // signature pref is honoured on ESR and DevEdition builds only;
        // official release builds ignore it and then require AMO signing.
        ("extensions.autoDisableScopes".into(), "0".into()),
        ("extensions.enabledScopes".into(), "15".into()),
        ("extensions.update.enabled".into(), "true".into()),
        ("extensions.update.autoUpdateDefault".into(), "true".into()),
        ("extensions.getAddons.cache.enabled".into(), "true".into()),
        ("xpinstall.signatures.required".into(), "false".into()),
        // Rendering: Firefox picks WebRender or software GL itself.
        // Forcing it breaks hosts without hardware GL (WSLg, containers).
        ("media.autoplay.default".into(), "0".into()),
        ("media.autoplay.blocking_policy".into(), "0".into()),
        ("dom.webnotifications.enabled".into(), "true".into()),
        ("dom.push.enabled".into(), "true".into()),
    ];
    if private {
        prefs.push(("browser.privatebrowsing.autostart".into(), "true".into()));
    }
    prefs.push((
        "remote.debugging.port".to_string(),
        port.to_string(),
    ));
    prefs
}

/// Write the `user.js` of a profile.
fn write_prefs(profile: &Path, port: u16, download_dir: &Path, private: bool) -> Result<(), String> {
    let mut out = String::from("// Generated by tontoo-webengine. Do not edit.\n");
    for (name, value) in profile_prefs(port, download_dir, private) {
        out.push_str(&format!("user_pref({name:?}, {value:?});\n"));
    }
    std::fs::write(profile.join("user.js"), out).map_err(|e| format!("user.js: {e}"))
}

/// Firefox launch arguments.
pub fn firefox_args(
    profile: &Path,
    port: u16,
    kiosk: bool,
    headless: bool,
    private: bool,
    window_size: Option<(u32, u32)>,
    start_url: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--profile".to_string(),
        profile.to_string_lossy().to_string(),
        "--new-instance".to_string(),
        "--no-remote".to_string(),
        "--remote-debugging-port".to_string(),
        port.to_string(),
    ];
    if kiosk && !headless {
        args.push("--kiosk".to_string());
    }
    if headless {
        args.push("--headless".to_string());
    }
    if private {
        args.push("--private-window".to_string());
    }
    if let Some((width, height)) = window_size.filter(|(w, h)| *w > 0 && *h > 0) {
        args.push(format!("--window-size={width},{height}"));
    }
    args.push(
        start_url
            .filter(|u| !u.trim().is_empty())
            .unwrap_or("about:blank")
            .to_string(),
    );
    args
}

/// Pick an unused loopback TCP port.
fn free_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("free port: {e}"))?;
    Ok(listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port())
}

/// Raw BiDi event (method + params) coming off a connection.
pub struct BidiRawEvent {
    pub method: String,
    pub params: JsonValue,
}

/// Helper-facing BiDi events after classification.
#[derive(Debug, Clone)]
pub enum BidiEvent {
    /// A top-level window appeared; `context` is its browsing context.
    ContextCreated { context: String },
    /// Navigation started for a context.
    NavigationStarted {
        context: String,
        url: Option<String>,
    },
    /// DOM parsed; user scripts run here.
    DomContentLoaded { context: String },
    /// The load event fired; the page is complete.
    Load { context: String },
    /// `window.open` / a new tab appeared.
    NewContext { context: String },
    /// A context went away (tab closed).
    ContextDestroyed { context: String },
    /// `alert`/`confirm`/`prompt`/`beforeunload` is waiting.
    UserPrompt {
        context: String,
        kind: String,
        message: String,
    },
    /// A download started; `download` is the BiDi download id.
    DownloadBegin {
        download: String,
        url: Option<String>,
        filename: String,
    },
    /// Download progress; `state` is `pending`/`completed`/`canceled`.
    DownloadProgress {
        download: String,
        state: String,
        received: u64,
    },
    /// A page called a registered message handler.
    ScriptMessage {
        channel: String,
        data: JsonValue,
    },
    /// `console.debug` traffic from the message fallback shim.
    ConsoleLine { text: String },
    /// An extension was installed or updated.
    ExtensionInstalled { id: String },
}

/// Classify one inbound BiDi event.
///
/// Download events are matched on their suffix so both the
/// `browsingContext.*` and the newer `network.*` spellings work.
fn classify(method: &str, params: &JsonValue) -> Option<BidiEvent> {
    let context = || {
        params
            .get("context")
            .and_then(|c| c.as_str())
            .map(str::to_string)
    };
    if let Some(rest) = method.strip_prefix("browsingContext.") {
        return classify_browsing_context(rest, params, &context);
    }
    Some(match method {
        "network.downloadWillBegin" => BidiEvent::DownloadBegin {
            download: params
                .get("download")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            url: params.get("url").and_then(|u| u.as_str()).map(str::to_string),
            filename: params
                .get("suggestedFilename")
                .and_then(|f| f.as_str())
                .unwrap_or("download")
                .to_string(),
        },
        "network.downloadProgress" => BidiEvent::DownloadProgress {
            download: params
                .get("download")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            state: params
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("pending")
                .to_string(),
            received: params
                .get("receivedBytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        },
        "log.entryAdded" => BidiEvent::ConsoleLine {
            text: params
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "script.message" => BidiEvent::ScriptMessage {
            channel: params
                .get("channel")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string(),
            data: params.get("data").cloned().unwrap_or_default(),
        },
        "webExtension.installed" => BidiEvent::ExtensionInstalled {
            id: params
                .get("extension")
                .and_then(|e| e.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        _ => return None,
    })
}

/// `browsingContext.*` sub-events. `context` is the accessor for the
/// `context` field, which only exists on some of them.
fn classify_browsing_context(
    event: &str,
    params: &JsonValue,
    context: &dyn Fn() -> Option<String>,
) -> Option<BidiEvent> {
    Some(match event {
        "contextCreated" => {
            let ctx = context()?;
            if params.get("parent").and_then(|p| p.as_str()).is_some() {
                BidiEvent::NewContext { context: ctx }
            } else {
                BidiEvent::ContextCreated { context: ctx }
            }
        }
        "contextDestroyed" => BidiEvent::ContextDestroyed {
            context: context()?,
        },
        "navigationStarted" => BidiEvent::NavigationStarted {
            context: context()?,
            url: params.get("url").and_then(|u| u.as_str()).map(str::to_string),
        },
        "domContentLoaded" => BidiEvent::DomContentLoaded {
            context: context()?,
        },
        "load" => BidiEvent::Load {
            context: context()?,
        },
        "userPromptOpened" => BidiEvent::UserPrompt {
            context: context()?,
            kind: params
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("alert")
                .to_string(),
            message: params
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "downloadWillBegin" => BidiEvent::DownloadBegin {
            download: params
                .get("download")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            url: params.get("url").and_then(|u| u.as_str()).map(str::to_string),
            filename: params
                .get("suggestedFilename")
                .and_then(|f| f.as_str())
                .unwrap_or("download")
                .to_string(),
        },
        "downloadProgress" => BidiEvent::DownloadProgress {
            download: params
                .get("download")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
            state: params
                .get("state")
                .and_then(|s| s.as_str())
                .unwrap_or("pending")
                .to_string(),
            received: params
                .get("receivedBytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        },
        _ => return None,
    })
}

/// One BiDi WebSocket connection.
pub struct BidiConn {
    socket: Mutex<WebSocket<MaybeTlsStream<std::net::TcpStream>>>,
    next_id: AtomicU32,
    pending: Mutex<HashMap<u32, SyncSender<Result<JsonValue, String>>>>,
    shutdown: AtomicBool,
}

impl BidiConn {
    /// Connect and pump incoming messages: replies route to `call`, events
    /// go to `events`. The reader thread runs until shutdown or disconnect.
    pub fn connect(
        url: &str,
        events: SyncSender<BidiRawEvent>,
    ) -> Result<Arc<Self>, String> {
        let (mut socket, _) =
            connect(url).map_err(|e| format!("bidi websocket connect: {e}"))?;
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        }
        let conn = Arc::new(Self {
            socket: Mutex::new(socket),
            next_id: AtomicU32::new(1),
            pending: Mutex::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
        });
        let worker = Arc::clone(&conn);
        let debug = std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some();
        let mut timeouts = 0u32;
        std::thread::spawn(move || loop {
            if worker.shutdown.load(Ordering::SeqCst) {
                break;
            }
            let text = {
                let mut socket = worker.socket.lock().expect("bidi socket lock");
                match socket.read() {
                    Ok(Message::Text(text)) => Some(text.to_string()),
                    Ok(_) => continue,
                    Err(tungstenite::Error::Io(e))
                        if e.kind() == std::io::ErrorKind::TimedOut
                            || e.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        timeouts += 1;
                        if debug && timeouts % 80 == 1 {
                            eprintln!("debug: bidi reader alive (timeouts={timeouts})");
                        }
                        continue
                    }
                    Err(_) => break,
                }
            };
            let Some(text) = text else { continue };
            match parse_message(&text) {
                Incoming::Reply { id, result, error } => {
                    if debug {
                        match &error {
                            Some(e) => eprintln!("debug: bidi reply {id} error: {e}"),
                            None => eprintln!("debug: bidi reply {id} ok"),
                        }
                        let _ = &result;
                    }
                    if let Some(tx) = worker.pending.lock().expect("pending lock").remove(&id) {
                        let _ = tx.try_send(match error {
                            Some(e) => Err(e),
                            None => Ok(result.unwrap_or(JsonValue::Null)),
                        });
                    }
                }
                Incoming::Event { method, params } => {
                    if debug {
                        eprintln!("debug: bidi event {method}");
                    }
                    let _ = events.try_send(BidiRawEvent { method, params });
                }
                Incoming::Other => {}
            }
        });
        Ok(conn)
    }

    /// Send a BiDi command and wait up to [`BIDI_TIMEOUT`].
    pub fn call(&self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        self.call_timeout(method, params, BIDI_TIMEOUT)
    }

    /// Send a BiDi command and wait at most `timeout`. Use a short
    /// timeout for calls that a build may simply never answer, so one
    /// silent command cannot stall a launch.
    pub fn call_timeout(
        &self,
        method: &str,
        params: JsonValue,
        timeout: Duration,
    ) -> Result<JsonValue, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = sync_channel(1);
        self.pending.lock().expect("pending lock").insert(id, tx);
        let line = method_call(id, method, &params);
        let debug = std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some();
        if debug {
            let mut shown = line.clone();
            if shown.len() > 400 {
                shown.truncate(400);
                shown.push_str("...");
            }
            eprintln!("debug: bidi send {method} id={id} {shown}");
        }
        {
            let mut socket = self.socket.lock().expect("bidi socket lock");
            socket
                .send(Message::Text(line.into()))
                .map_err(|e| format!("bidi websocket send: {e}"))?;
        }
        let deadline = Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("bidi connection closed".into())
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(format!("bidi call timed out: {method}"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Close the connection and stop the reader thread.
    pub fn close(&self) {
        use std::net::Shutdown;
        self.shutdown.store(true, Ordering::SeqCst);
        if let Ok(mut socket) = self.socket.lock() {
            let _ = socket.close(None);
            if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// Classified inbound BiDi message.
pub enum Incoming {
    Reply {
        id: u32,
        result: Option<JsonValue>,
        error: Option<String>,
    },
    Event {
        method: String,
        params: JsonValue,
    },
    Other,
}

/// Parse one inbound BiDi text message.
pub(crate) fn parse_message(text: &str) -> Incoming {
    let value: JsonValue = match JsonValue::parse(text) {
        Ok(v) => v,
        Err(_) => return Incoming::Other,
    };
    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
        if value.get("method").is_some() {
            return Incoming::Other;
        }
        // BiDi replies carry `type: "success" | "error"`; error replies
        // have a code (`error`) plus a human message.
        let error = match value.get("type").and_then(|t| t.as_str()) {
            Some("error") => {
                let code = value.get("error").and_then(|e| e.as_str());
                match (code, value.get("message").and_then(|m| m.as_str())) {
                    (Some(code), Some(message)) if !message.is_empty() => {
                        Some(format!("{code}: {message}"))
                    }
                    (Some(code), _) => Some(code.to_string()),
                    (None, Some(message)) if !message.is_empty() => Some(message.to_string()),
                    (None, _) => Some("bidi error".to_string()),
                }
            }
            _ => None,
        };
        return Incoming::Reply {
            id: id as u32,
            result: value.get("result").cloned(),
            error,
        };
    }
    match value.get("method").and_then(|m| m.as_str()) {
        Some(method) => Incoming::Event {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or_default(),
        },
        None => Incoming::Other,
    }
}

/// Build a BiDi command line.
pub(crate) fn method_call(id: u32, method: &str, params: &JsonValue) -> String {
    JsonValue::Object(vec![
        ("id".to_string(), JsonValue::Integer(id as i64)),
        ("method".to_string(), JsonValue::Str(method.to_string())),
        ("params".to_string(), params.clone()),
    ])
    .to_compact_string()
}

/// Standard base64 alphabet encoder (used for `data:` URLs and `xpi`
/// payloads, where BiDi expects base64 rather than percent encoding).
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// `data:text/html;base64,...` URL for raw HTML.
pub fn html_data_url(html: &str) -> String {
    format!(
        "data:text/html;charset=utf-8;base64,{}",
        base64_encode(html.as_bytes())
    )
}

/// A BiDi remote object flattened to plain JSON.
///
/// BiDi wraps values in `{type, value}`; strings, numbers, booleans,
/// arrays and maps are unwrapped recursively. Missing values, `undefined`
/// and non-serializable numbers (`NaN`, infinities) become null, as do
/// types that have no JSON form (nodes, windows, functions).
pub fn remote_to_json(value: &JsonValue) -> JsonValue {
    match value.get("type").and_then(|t| t.as_str()) {
        Some("undefined") | Some("null") => return JsonValue::Null,
        Some("number") | Some("bigint") | Some("string") | Some("boolean") => {
            return value.get("value").cloned().unwrap_or(JsonValue::Null)
        }
        Some("array") => {
            return match value.get("value").and_then(|v| v.as_array()) {
                Some(list) => JsonValue::Array(list.iter().map(remote_to_json).collect()),
                None => JsonValue::Null,
            }
        }
        Some("map") | Some("object") => {
            return match value.get("value") {
                Some(inner) => remote_to_json(inner),
                None => JsonValue::Null,
            }
        }
        Some(_) => return JsonValue::Null,
        None => {}
    }
    // Already a plain JSON value.
    if let Some(list) = value.as_array() {
        return JsonValue::Array(list.iter().map(remote_to_json).collect());
    }
    if value.is_object() {
        let mut out = Vec::new();
        if let Some(entries) = value.object_entries() {
            for (key, entry) in entries {
                out.push((key.clone(), remote_to_json(entry)));
            }
        }
        return JsonValue::Object(out);
    }
    value.clone()
}

/// Bootstrap injected into every document: defines
/// `window.tontoo.postMessage(name, body)` and the legacy
/// `window.webkit.messageHandlers.<name>.postMessage(body)` shape.
pub const MESSAGE_BOOTSTRAP: &str = r#"
(function () {
  if (window.tontoo) { return; }
  var handlers = Object.create(null);
  function post(name, body) {
    var payload = JSON.stringify({ name: name, body: body === undefined ? null : body });
    try {
      console.debug("__tontoo__" + payload);
    } catch (e) {}
  }
  window.tontoo = { postMessage: post, post: post };
  if (!window.webkit) { window.webkit = {}; }
  if (!window.webkit.messageHandlers) { window.webkit.messageHandlers = {}; }
  var shim = new Proxy({}, {
    get: function (_, name) {
      if (typeof name !== "string") { return undefined; }
      if (!handlers[name]) {
        handlers[name] = { postMessage: function (body) { post(name, body); } };
      }
      return handlers[name];
    }
  });
  try {
    Object.defineProperty(window.webkit, "messageHandlers", {
      value: shim,
      configurable: true
    });
  } catch (e) {
    window.webkit.messageHandlers = shim;
  }
})();
"#;

/// Extract the page message from a `console.debug` line, if any.
pub fn parse_console_message(line: &str) -> Option<(String, JsonValue)> {
    let payload = line.trim().strip_prefix("__tontoo__")?;
    let value = JsonValue::parse(payload).ok()?;
    let name = value.get("name")?.as_str()?.to_string();
    Some((name, value.get("body").cloned().unwrap_or(JsonValue::Null)))
}

/// One extension the managed profile installs at startup.
///
/// This is Firefox's supported managed-install mechanism
/// (`<profile>/policies.json`), so it works on every build, including
/// those whose WebDriver BiDi build has no `webExtension` module.
#[derive(Debug, Clone)]
pub enum ExtensionPolicy {
    /// Install this AMO add-on id; Firefox downloads it and keeps it
    /// updated itself. Signing is enforced, so use ids from AMO.
    AddonId(String),
    /// Install this `.xpi` URL at startup.
    Url { url: String },
}

/// Merge `entries` into `<profile>/policies.json`.
///
/// Existing keys are kept: the file is read, the `3rdparty.Extensions`
/// object is extended and written back. Returns an error when the file
/// exists but is not valid JSON.
pub fn write_extension_policy(
    profile: &Path,
    entries: &[ExtensionPolicy],
) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let path = profile.join("policies.json");
    let mut policies = match std::fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => JsonValue::parse(&text)
            .map_err(|e| format!("policies.json: {e}"))?,
        _ => JsonValue::Object(vec![]),
    };
    let mut extensions = match policies
        .get("policies")
        .and_then(|p| p.get("3rdparty"))
        .and_then(|t| t.get("Extensions"))
    {
        Some(JsonValue::Object(existing)) => existing.clone(),
        _ => Vec::new(),
    };
    for entry in entries {
        let (id, value) = match entry {
            ExtensionPolicy::AddonId(id) => (
                id.clone(),
                JsonValue::Object(vec![
                    (
                        "installation_mode".to_string(),
                        JsonValue::Str("normal_installed".to_string()),
                    ),
                ]),
            ),
            ExtensionPolicy::Url { url } => (
                url.clone(),
                JsonValue::Object(vec![
                    ("URL".to_string(), JsonValue::Str(url.clone())),
                    (
                        "installation_mode".to_string(),
                        JsonValue::Str("normal_installed".to_string()),
                    ),
                ]),
            ),
        };
        match extensions.iter_mut().find(|(key, _)| *key == id) {
            Some(slot) => slot.1 = value,
            None => extensions.push((id, value)),
        }
    }
    policies = merge_json_object(
        policies,
        &["policies", "3rdparty", "Extensions"],
        extensions,
    );
    std::fs::write(&path, policies.to_compact_string()).map_err(|e| format!("policies.json: {e}"))
}

/// Replace (or insert) a nested key path in a JSON object.
fn merge_json_object(
    root: JsonValue,
    path: &[&str],
    value: Vec<(String, JsonValue)>,
) -> JsonValue {
    let (key, rest) = match path.split_first() {
        Some(parts) => parts,
        None => return root,
    };
    let mut entries = match &root {
        JsonValue::Object(existing) => existing.clone(),
        _ => Vec::new(),
    };
    let merged = if rest.is_empty() {
        JsonValue::Object(value)
    } else {
        let current = entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or(JsonValue::Object(Vec::new()));
        merge_json_object(current, rest, value)
    };
    match entries.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = merged,
        None => entries.push(((*key).to_string(), merged)),
    }
    JsonValue::Object(entries)
}

/// Launch options for [`GeckoPage::launch`].
#[derive(Debug, Clone)]
pub struct GeckoOptions {
    /// Ask Firefox for fullscreen chrome-less mode (`--kiosk`).
    ///
    /// Off by default: `--kiosk` sends an `xdg_toplevel.fullscreen`
    /// request, and a compositor that answers it with a 0 x 0 size makes
    /// Firefox abort with `xdg_surface buffer (1 x 1) is larger than the
    /// configured fullscreen state (0 x 0)`. Turn it on only when the
    /// compositor implements fullscreen properly.
    pub kiosk: bool,
    /// No window at all (tests and headless servers).
    pub headless: bool,
    /// Ephemeral profile, deleted on exit.
    pub private: bool,
    /// Where Firefox writes downloads.
    pub download_dir: PathBuf,
    /// Initial window size in logical px (`--window-size=W,H`).
    pub window_size: Option<(u32, u32)>,
    /// Page to open at startup.
    pub start_url: Option<String>,
    /// Download a Firefox build when none exists. `true` for apps that
    /// want a real engine and can wait for it; `false` when the caller
    /// provisions in the background (the `tontoo-webengine` helper) or has
    /// a fallback engine.
    pub provision: bool,
    /// Add-ons the profile installs at startup through
    /// [`write_extension_policy`].
    pub extensions: Vec<ExtensionPolicy>,
}

impl Default for GeckoOptions {
    fn default() -> Self {
        Self {
            kiosk: false,
            headless: false,
            private: false,
            download_dir: std::env::temp_dir(),
            window_size: None,
            start_url: None,
            provision: true,
            extensions: Vec::new(),
        }
    }
}

/// One driven Firefox: owns the browser child, the BiDi connection and the
/// profile dir. Classified events arrive on the receiver returned by
/// [`GeckoPage::launch`].
pub struct GeckoPage {
    child: Mutex<Child>,
    profile: PathBuf,
    private_profile: bool,
    conn: Arc<BidiConn>,
    context: RwLock<Option<String>>,
    title: RwLock<Option<String>>,
    closed: AtomicBool,
}

impl GeckoPage {
    /// Spawn Firefox and attach a BiDi session.
    ///
    /// With `options.provision` (the default) a missing browser is
    /// downloaded and unpacked here, so the first launch already has a
    /// real engine. Otherwise only [`find_firefox`] is used and the caller
    /// can fall back to another engine.
    pub fn launch(options: GeckoOptions) -> Result<(Self, Receiver<BidiEvent>), String> {
        let bin = if options.provision {
            find_firefox().or_else(|e| {
                eprintln!("tontoo-webengine: {e}; provisioning a Firefox build...");
                super::gecko_provision::ensure_firefox()
            })?
        } else {
            find_firefox()?
        };
        let port = free_port()?;
        let profile = profile_dir(options.private)?;
        std::fs::create_dir_all(&options.download_dir)
            .map_err(|e| format!("download dir: {e}"))?;
        write_prefs(&profile, port, &options.download_dir, options.private)?;
        write_extension_policy(&profile, &options.extensions)?;

        let args = firefox_args(
            &profile,
            port,
            options.kiosk,
            options.headless,
            options.private,
            options.window_size,
            options.start_url.as_deref(),
        );
        eprintln!("tontoo-webengine: launching {}", bin.display());
        let child = Command::new(&bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn firefox: {e}"))?;

        let (raw_tx, raw_rx) = sync_channel::<BidiRawEvent>(1024);
        let (event_tx, event_rx) = sync_channel::<BidiEvent>(1024);
        std::thread::spawn(move || {
            while let Ok(raw) = raw_rx.recv() {
                if let Some(event) = classify(&raw.method, &raw.params) {
                    if event_tx.try_send(event).is_err() {
                        break;
                    }
                }
            }
        });

        let conn = Self::connect_with_retry(port, &raw_tx)?;
        let page = Self {
            child: Mutex::new(child),
            profile,
            private_profile: options.private,
            conn,
            context: RwLock::new(None),
            title: RwLock::new(None),
            closed: AtomicBool::new(false),
        };
        page.start_session()?;
        Ok((page, event_rx))
    }

    /// `session.new` and the initial event subscription.
    fn start_session(&self) -> Result<(), String> {
        self.call(
            "session.new",
            JsonValue::Object(vec![("capabilities".to_string(), JsonValue::Object(vec![]))]),
        )?;
        self.subscribe_events();
        self.install_message_bridge();
        self.refresh_context()?;
        Ok(())
    }

    /// Subscribe to [`SUBSCRIBED_EVENTS`], one event per call.
    ///
    /// Per-event calls are deliberate: Firefox rejects the whole call when
    /// one name is unknown ("x is not a valid event name"), and some builds
    /// never answer a multi-event subscribe at all. Names a build does not
    /// know are reported once and skipped.
    fn subscribe_events(&self) {
        static NOTED: std::sync::Once = std::sync::Once::new();
        for name in SUBSCRIBED_EVENTS {
            let one = JsonValue::Array(vec![JsonValue::Str(name.to_string())]);
            if let Err(e) = self.call_timeout(
                "session.subscribe",
                JsonValue::Object(vec![("events".to_string(), one)]),
                SUBSCRIBE_TIMEOUT,
            ) {
                NOTED.call_once(|| {
                    eprintln!("tontoo-webengine: this Firefox has no {name} event ({e})")
                });
            }
        }
    }

    /// Wait for the remote agent, then open the BiDi WebSocket.
    ///
    /// The port is polled with a short `connect_timeout` first: once the
    /// agent accepts TCP connections it also answers the HTTP upgrade
    /// immediately, so this avoids handing a half-open socket to
    /// tungstenite, whose `connect` has no timeout of its own.
    fn connect_with_retry(
        port: u16,
        events: &SyncSender<BidiRawEvent>,
    ) -> Result<Arc<BidiConn>, String> {
        let url = format!("ws://127.0.0.1:{port}/session");
        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        let mut last = String::new();
        while Instant::now() < deadline {
            match std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}")
                    .parse()
                    .expect("loopback address parses"),
                Duration::from_millis(500),
            ) {
                Ok(_) => match BidiConn::connect(&url, events.clone()) {
                    Ok(conn) => return Ok(conn),
                    Err(e) => last = e,
                },
                Err(e) => last = format!("remote agent not accepting yet: {e}"),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("bidi endpoint never came up ({last})"))
    }

    /// Register the preload script that bridges page messages to BiDi.
    ///
    /// `script.addPreloadScript` is the designed channel; when the build
    /// does not have it (older ESR) the bootstrap is injected after every
    /// load and messages arrive as `log.entryAdded` console lines.
    fn install_message_bridge(&self) {
        let params = JsonValue::Object(vec![
            (
                "functions".to_string(),
                JsonValue::Array(vec![JsonValue::Object(vec![(
                    "functionDeclaration".to_string(),
                    JsonValue::Str(MESSAGE_BOOTSTRAP.to_string()),
                )])]),
            ),
            ("channel".to_string(), JsonValue::Str("tontoo".to_string())),
        ]);
        match self.call("script.addPreloadScript", params) {
            Ok(_) => {}
            Err(e) => eprintln!(
                "tontoo-webengine: script.addPreloadScript unavailable ({e}); \
                 falling back to post-load injection"
            ),
        }
        // Older builds ignore the channel subscription; the console
        // fallback needs the log stream either way.
        let _ = self.call(
            "session.subscribe",
            JsonValue::Object(vec![(
                "events".to_string(),
                JsonValue::Array(vec![JsonValue::Str("script.message".to_string())]),
            )]),
        );
    }

    /// Firefox process id (the parent), for window lookups.
    pub fn pid(&self) -> u32 {
        self.child.lock().map(|c| c.id()).unwrap_or(0)
    }

    /// Directory where Firefox writes its crash reports
    /// (`<profile>/crashes`).
    pub fn crash_dir(&self) -> PathBuf {
        self.profile.join("crashes")
    }

    /// Whether the browser process is gone, with the exit reason.
    ///
    /// On Unix a signal shows up as `signal N`; that is what a Wayland
    /// protocol error looks like, because the client aborts on it.
    pub fn exit_reason(&self) -> Option<String> {
        use std::os::unix::process::ExitStatusExt;
        let mut child = self.child.lock().ok()?;
        let status = child.try_wait().ok()??;
        Some(if let Some(signal) = status.signal() {
            format!("killed by signal {signal}")
        } else if status.success() {
            "exited normally".to_string()
        } else {
            format!("exited with {}", status)
        })
    }

    /// Turn a dead connection into an actionable message.
    ///
    /// A raw "closed connection" hides the interesting part: a Firefox
    /// that died (crash, protocol error, user closed every window) is
    /// reported with its exit reason and the crash-report directory.
    fn enrich_error(&self, error: String) -> String {
        let dead = error.contains("closed connection")
            || error.contains("Connection reset")
            || error.contains("Broken pipe")
            || error.contains("websocket send");
        if !dead {
            return error;
        }
        match self.exit_reason() {
            Some(reason) => format!(
                "Firefox {reason}; crash reports in {}",
                self.crash_dir().display()
            ),
            None => format!("{error} (Firefox is still running)"),
        }
    }

    /// BiDi call with enriched error messages.
    fn call(&self, method: &str, params: JsonValue) -> Result<JsonValue, String> {
        self.conn
            .call(method, params)
            .map_err(|e| self.enrich_error(e))
    }

    /// BiDi call with a short timeout and enriched error messages.
    fn call_timeout(
        &self,
        method: &str,
        params: JsonValue,
        timeout: Duration,
    ) -> Result<JsonValue, String> {
        self.conn
            .call_timeout(method, params, timeout)
            .map_err(|e| self.enrich_error(e))
    }

    /// The raw BiDi connection (advanced use).
    pub fn connection(&self) -> &Arc<BidiConn> {
        &self.conn
    }

    /// The active top-level browsing context, once known.
    pub fn context(&self) -> Option<String> {
        self.context.read().ok().and_then(|c| c.clone())
    }

    /// Set the active browsing context (e.g. after a tab switch).
    pub fn set_context(&self, context: Option<String>) {
        if let Ok(mut slot) = self.context.write() {
            *slot = context;
        }
    }

    fn context_or_err(&self) -> Result<String, String> {
        self.context()
            .ok_or_else(|| "no browsing context yet".to_string())
    }

    /// Re-read the top-level context from the context tree.
    pub fn refresh_context(&self) -> Result<String, String> {
        let tree = self.call(
            "browsingContext.getTree",
            JsonValue::Object(vec![]),
        )?;
        let contexts = tree
            .get("contexts")
            .and_then(|c| c.as_array())
            .ok_or_else(|| "getTree without contexts".to_string())?;
        let top = contexts
            .first()
            .and_then(|entry| entry.get("context"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "getTree without a top-level context".to_string())?
            .to_string();
        self.set_context(Some(top.clone()));
        Ok(top)
    }

    /// Navigate the active context. Returns as soon as the navigation is
    /// accepted; `browsingContext.load` reports completion.
    pub fn navigate(&self, url: &str) -> Result<(), String> {
        let context = self.context_or_err()?;
        self.call(
            "browsingContext.navigate",
            JsonValue::Object(vec![
                ("context".to_string(), JsonValue::Str(context)),
                ("url".to_string(), JsonValue::Str(url.to_string())),
                ("wait".to_string(), JsonValue::Str("none".to_string())),
            ]),
        )?;
        Ok(())
    }

    /// Navigate to a `data:` URL carrying raw HTML.
    pub fn load_html(&self, html: &str, base_uri: Option<&str>) -> Result<(), String> {
        let mut url = html_data_url(html);
        if let Some(base) = base_uri.filter(|b| !b.trim().is_empty()) {
            // Firefox ignores <base href> for data: documents; encode the
            // hint as a meta element so relative resources still resolve.
            let injected = format!(
                "<base href=\"{}\">{}",
                base.replace('"', "&quot;"),
                html
            );
            url = html_data_url(&injected);
        }
        self.navigate(&url)
    }

    /// Reload, optionally bypassing the cache.
    pub fn reload(&self, ignore_cache: bool) -> Result<(), String> {
        let context = self.context_or_err()?;
        self.call(
            "browsingContext.reload",
            JsonValue::Object(vec![
                ("context".to_string(), JsonValue::Str(context)),
                ("ignoreCache".to_string(), JsonValue::Bool(ignore_cache)),
                ("wait".to_string(), JsonValue::Str("none".to_string())),
            ]),
        )?;
        Ok(())
    }

    /// Stop loading the current document.
    ///
    /// WebDriver BiDi has no abort-navigation command, so this only asks
    /// the page to stop fetching: the pending `browsingContext.load` event
    /// still arrives and the helper reports the finished load.
    pub fn stop(&self) -> Result<(), String> {
        self.evaluate("window.stop()")?;
        Ok(())
    }

    /// Session history entries plus the current index.
    pub fn history(&self) -> Result<(Vec<String>, usize), String> {
        let context = self.context_or_err()?;
        let value = self.call(
            "browsingContext.history",
            JsonValue::Object(vec![("context".to_string(), JsonValue::Str(context))]),
        )?;
        let index = value.get("currentIndex").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let mut urls = Vec::new();
        if let Some(entries) = value.get("entries").and_then(|e| e.as_array()) {
            for entry in entries {
                if let Some(url) = entry.get("url").and_then(|u| u.as_str()) {
                    urls.push(url.to_string());
                }
            }
        }
        Ok((urls, index))
    }

    /// History availability for the back/forward controls.
    pub fn history_state(&self) -> Result<(bool, bool), String> {
        let (urls, index) = self.history()?;
        let can_back = index > 0;
        let can_forward = index + 1 < urls.len();
        Ok((can_back, can_forward))
    }

    /// Go `delta` entries back (negative) or forward (positive).
    pub fn go_history(&self, delta: i32) -> Result<(), String> {
        let (urls, index) = self.history()?;
        let target = index as i64 + delta as i64;
        if target < 0 || target as usize >= urls.len() {
            return Err("history index out of range".into());
        }
        self.navigate(&urls[target as usize])
    }

    /// Evaluate JavaScript in the active context and return a plain value.
    pub fn evaluate(&self, expression: &str) -> Result<JsonValue, String> {
        let context = self.context_or_err()?;
        let value = self.call(
            "script.evaluate",
            JsonValue::Object(vec![
                ("expression".to_string(), JsonValue::Str(expression.to_string())),
                (
                    "target".to_string(),
                    JsonValue::Object(vec![("context".to_string(), JsonValue::Str(context))]),
                ),
                ("awaitPromise".to_string(), JsonValue::Bool(true)),
                ("resultOwnership".to_string(), JsonValue::Str("none".to_string())),
                ("userActivation".to_string(), JsonValue::Bool(true)),
            ]),
        )?;
        let remote = value.get("result").unwrap_or(&JsonValue::Null);
        if remote.get("type").and_then(|t| t.as_str()) == Some("exception") {
            let detail = remote
                .get("exceptionDetails")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("script exception");
            return Err(detail.to_string());
        }
        Ok(remote_to_json(remote))
    }

    /// Cache and return the current document title.
    pub fn title(&self) -> Option<String> {
        if let Ok(title) = self.evaluate("document.title") {
            if let Some(text) = title.as_str() {
                let text = text.to_string();
                if let Ok(mut slot) = self.title.write() {
                    *slot = Some(text.clone());
                }
                return Some(text);
            }
        }
        self.title.read().ok().and_then(|t| t.clone())
    }

    /// Inject a script into the current document (user scripts, shims).
    pub fn inject(&self, script: &str) -> Result<JsonValue, String> {
        self.evaluate(script)
    }

    fn pointer_action(&self, x: f64, y: f64, actions: Vec<JsonValue>) -> Result<(), String> {
        let context = self.context_or_err()?;
        let mut list = vec![JsonValue::Object(vec![
            ("type".to_string(), JsonValue::Str("pointerMove".to_string())),
            ("x".to_string(), JsonValue::Float(x)),
            ("y".to_string(), JsonValue::Float(y)),
            ("duration".to_string(), JsonValue::Integer(0)),
        ])];
        list.extend(actions);
        self.call(
            "input.performActions",
            JsonValue::Object(vec![
                ("context".to_string(), JsonValue::Str(context)),
                (
                    "actions".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(vec![
                        ("type".to_string(), JsonValue::Str("pointer".to_string())),
                        ("id".to_string(), JsonValue::Str("mouse".to_string())),
                        ("actions".to_string(), JsonValue::Array(list)),
                    ])]),
                ),
            ]),
        )?;
        Ok(())
    }

    fn key_actions(&self, actions: Vec<JsonValue>) -> Result<(), String> {
        let context = self.context_or_err()?;
        self.call(
            "input.performActions",
            JsonValue::Object(vec![
                ("context".to_string(), JsonValue::Str(context)),
                (
                    "actions".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(vec![
                        ("type".to_string(), JsonValue::Str("key".to_string())),
                        ("id".to_string(), JsonValue::Str("keyboard".to_string())),
                        ("actions".to_string(), JsonValue::Array(actions)),
                    ])]),
                ),
            ]),
        )?;
        Ok(())
    }

    /// Move the pointer to a page coordinate.
    pub fn pointer_move(&self, x: f64, y: f64) -> Result<(), String> {
        self.pointer_action(x, y, Vec::new())
    }

    /// Press or release the left button at a page coordinate.
    pub fn pointer_button(&self, x: f64, y: f64, pressed: bool) -> Result<(), String> {
        let kind = if pressed { "pointerDown" } else { "pointerUp" };
        self.pointer_action(
            x,
            y,
            vec![JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str(kind.to_string())),
                ("button".to_string(), JsonValue::Integer(0)),
            ])],
        )
    }

    /// Wheel scroll by a delta in CSS pixels.
    pub fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) -> Result<(), String> {
        self.pointer_action(
            x,
            y,
            vec![JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str("scroll".to_string())),
                ("x".to_string(), JsonValue::Float(x)),
                ("y".to_string(), JsonValue::Float(y)),
                ("deltaX".to_string(), JsonValue::Float(dx)),
                ("deltaY".to_string(), JsonValue::Float(dy)),
                ("duration".to_string(), JsonValue::Integer(0)),
            ])],
        )
    }

    /// Type text one character at a time.
    pub fn insert_text(&self, text: &str) -> Result<(), String> {
        let mut actions = Vec::new();
        for ch in text.chars() {
            actions.push(JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str("keyDown".to_string())),
                ("value".to_string(), JsonValue::Str(ch.to_string())),
            ]));
            actions.push(JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str("keyUp".to_string())),
                ("value".to_string(), JsonValue::Str(ch.to_string())),
            ]));
        }
        if actions.is_empty() {
            return Ok(());
        }
        self.key_actions(actions)
    }

    /// Press and release one key by its engine name (`"Enter"`, ...).
    pub fn press_key(&self, key: &str) -> Result<(), String> {
        let Some(value) = bidi_key_value(key) else {
            // Unknown names are treated as literal text.
            return self.insert_text(key);
        };
        self.key_actions(vec![
            JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str("keyDown".to_string())),
                ("value".to_string(), JsonValue::Str(value.to_string())),
            ]),
            JsonValue::Object(vec![
                ("type".to_string(), JsonValue::Str("keyUp".to_string())),
                ("value".to_string(), JsonValue::Str(value.to_string())),
            ]),
        ])
    }

    /// Answer an open user prompt (`alert`/`confirm`/`prompt`).
    pub fn handle_user_prompt(
        &self,
        context: &str,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<(), String> {
        let mut params = vec![
            ("context".to_string(), JsonValue::Str(context.to_string())),
            ("accept".to_string(), JsonValue::Bool(accept)),
        ];
        if let Some(text) = prompt_text {
            params.push(("promptText".to_string(), JsonValue::Str(text.to_string())));
        }
        self.conn
            .call("browsingContext.handleUserPrompt", JsonValue::Object(params))?;
        Ok(())
    }

    /// Set the emulated viewport (used by the headless self test; a
    /// headful Firefox window is sized by the compositor instead).
    pub fn set_viewport(&self, width: u32, height: u32, scale: f32) -> Result<(), String> {
        let context = self.context_or_err()?;
        self.call(
            "browsingContext.setViewport",
            JsonValue::Object(vec![
                ("context".to_string(), JsonValue::Str(context)),
                (
                    "viewport".to_string(),
                    JsonValue::Object(vec![
                        ("width".to_string(), JsonValue::Integer(width as i64)),
                        ("height".to_string(), JsonValue::Integer(height as i64)),
                    ]),
                ),
                (
                    "devicePixelRatio".to_string(),
                    JsonValue::Float(scale as f64),
                ),
            ]),
        )?;
        Ok(())
    }

    /// Open a new top-level window or tab.
    pub fn create_context(&self, kind: &str) -> Result<String, String> {
        let value = self.call(
            "browsingContext.create",
            JsonValue::Object(vec![("type".to_string(), JsonValue::Str(kind.to_string()))]),
        )?;
        value
            .get("context")
            .and_then(|c| c.as_str())
            .map(str::to_string)
            .ok_or_else(|| "create without context".into())
    }

    /// Close a browsing context (tab or window).
    pub fn close_context(&self, context: &str) -> Result<(), String> {
        self.call(
            "browsingContext.close",
            JsonValue::Object(vec![("context".to_string(), JsonValue::Str(context.to_string()))]),
        )?;
        Ok(())
    }

    /// Focus a context so its window comes to the front.
    pub fn activate_context(&self, context: &str) -> Result<(), String> {
        self.call(
            "browsingContext.activate",
            JsonValue::Object(vec![("context".to_string(), JsonValue::Str(context.to_string()))]),
        )?;
        Ok(())
    }

    /// Install a WebExtension from a local `manifest.json` directory or an
    /// `.xpi` archive and return its Firefox extension id.
    pub fn install_extension(&self, path: &Path) -> Result<String, String> {
        let absolute = std::fs::canonicalize(path)
            .map_err(|e| format!("extension path {}: {e}", path.display()))?;
        let value = self.call(
            "webExtension.install",
            JsonValue::Object(vec![(
                "extensionData".to_string(),
                JsonValue::Object(vec![
                    ("type".to_string(), JsonValue::Str("path".to_string())),
                    (
                        "path".to_string(),
                        JsonValue::Str(absolute.to_string_lossy().to_string()),
                    ),
                ]),
            )]),
        )?;
        let id = value
            .get("extension")
            .and_then(|e| e.as_str())
            .map(str::to_string)
            .ok_or_else(|| "webExtension.install without an id".to_string())?;
        Ok(id)
    }

    /// Installed extension ids as reported by Firefox.
    pub fn list_extensions(&self) -> Result<Vec<String>, String> {
        let value = self.call("webExtension.listExtensions", JsonValue::Object(vec![]))?;
        let mut ids = Vec::new();
        if let Some(list) = value.get("extensions").and_then(|e| e.as_array()) {
            for entry in list {
                if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// All cookies of the store, optionally filtered by domain.
    pub fn list_cookies(&self, domain: Option<&str>) -> Result<Vec<crate::cookie::Cookie>, String> {
        let mut filter = Vec::new();
        if let Some(domain) = domain.filter(|d| !d.trim().is_empty()) {
            filter.push(("domain".to_string(), JsonValue::Str(domain.to_string())));
        }
        let value = self.call(
            "storage.getCookies",
            JsonValue::Object(vec![("filter".to_string(), JsonValue::Object(filter))]),
        )?;
        let mut out = Vec::new();
        if let Some(list) = value.get("cookies").and_then(|c| c.as_array()) {
            for entry in list {
                let name = entry.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let raw = entry.get("value").unwrap_or(&JsonValue::Null);
                let text = raw
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        raw.get("value")
                            .map(|v| JsonValue::parse(&v.to_compact_string()).ok())
                            .flatten()
                            .and_then(|v| v.as_str().map(str::to_string))
                    })
                    .unwrap_or_default();
                out.push(crate::cookie::Cookie {
                    name: name.to_string(),
                    value: text,
                    domain: entry
                        .get("domain")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    path: entry
                        .get("path")
                        .and_then(|p| p.as_str())
                        .unwrap_or("/")
                        .to_string(),
                    secure: entry
                        .get("secure")
                        .and_then(|s| s.as_bool())
                        .unwrap_or(false),
                    http_only: entry
                        .get("httpOnly")
                        .and_then(|s| s.as_bool())
                        .unwrap_or(false),
                    expires: entry
                        .get("expiry")
                        .and_then(|e| e.as_f64())
                        .filter(|e| *e > 0.0)
                        .map(|e| e as i64),
                });
            }
        }
        Ok(out)
    }

    /// Add or replace one cookie.
    pub fn add_cookie(&self, cookie: &crate::cookie::Cookie) -> Result<(), String> {
        let mut fields = vec![
            ("name".to_string(), JsonValue::Str(cookie.name.clone())),
            (
                "value".to_string(),
                JsonValue::Object(vec![
                    ("type".to_string(), JsonValue::Str("string".to_string())),
                    ("value".to_string(), JsonValue::Str(cookie.value.clone())),
                ]),
            ),
            ("domain".to_string(), JsonValue::Str(cookie.domain.clone())),
            ("path".to_string(), JsonValue::Str(cookie.path.clone())),
            ("secure".to_string(), JsonValue::Bool(cookie.secure)),
            ("httpOnly".to_string(), JsonValue::Bool(cookie.http_only)),
        ];
        if let Some(expires) = cookie.expires {
            fields.push(("expiry".to_string(), JsonValue::Float(expires as f64)));
        }
        self.call(
            "storage.setCookie",
            JsonValue::Object(vec![("cookie".to_string(), JsonValue::Object(fields))]),
        )?;
        Ok(())
    }

    /// Delete every cookie matching name, domain and path.
    pub fn delete_cookie(&self, domain: &str, path: &str, name: &str) -> Result<(), String> {
        self.call(
            "storage.deleteCookies",
            JsonValue::Object(vec![(
                "filter".to_string(),
                JsonValue::Object(vec![
                    ("name".to_string(), JsonValue::Str(name.to_string())),
                    ("domain".to_string(), JsonValue::Str(domain.to_string())),
                    ("path".to_string(), JsonValue::Str(path.to_string())),
                ]),
            )]),
        )?;
        Ok(())
    }

    /// Delete all cookies and the HTTP cache.
    pub fn clear_data(&self) -> Result<(), String> {
        self.call(
            "storage.getCookies",
            JsonValue::Object(vec![]),
        )?;
        self.call(
            "network.clearData",
            JsonValue::Object(vec![
                ("dataTypes".to_string(), JsonValue::Array(vec![
                    JsonValue::Str("cookies".to_string()),
                    JsonValue::Str("cache".to_string()),
                    JsonValue::Str("cacheStorage".to_string()),
                    JsonValue::Str("localStorage".to_string()),
                    JsonValue::Str("indexedDB".to_string()),
                    JsonValue::Str("serviceWorkers".to_string()),
                ])),
            ]),
        )?;
        Ok(())
    }

    /// Ask Firefox to quit; the child is reaped on drop.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.call("browser.close", JsonValue::Object(vec![]));
        self.conn.close();
        if let Ok(mut child) = self.child.lock() {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        if self.private_profile {
            let _ = std::fs::remove_dir_all(&self.profile);
        }
    }
}

impl Drop for GeckoPage {
    fn drop(&mut self) {
        self.close();
    }
}

/// Bookkeeping shared by the in-process [`GeckoEngine`] and the helper
/// renderer: current URL, input position, download bookkeeping, user
/// scripts and the ids the engine protocol correlates on.
pub struct GeckoState {
    /// Current document URL.
    pub url: Option<String>,
    /// Last pointer position, used by wheel events.
    pub last_pos: (f64, f64),
    /// Next engine-local id for dialogs, downloads and requests.
    pub next_id: u64,
    /// Engine download id -> (BiDi download id, chosen destination).
    pub downloads: HashMap<u64, (String, Option<String>)>,
    /// BiDi download id -> engine download id.
    pub bidi_ids: HashMap<String, u64>,
    /// Where Firefox writes downloads before the delegate moves them.
    pub download_dir: PathBuf,
    /// Whether Firefox runs in kiosk mode.
    pub kiosk: bool,
    /// Firefox process id, reported as `EngineWindow`.
    pub pid: u32,
    /// User scripts injected after every load.
    pub scripts: Vec<String>,
}

impl GeckoState {
    /// Fresh state for one Firefox session.
    pub fn new(page: &GeckoPage, download_dir: PathBuf, kiosk: bool) -> Self {
        Self {
            url: None,
            last_pos: (0.0, 0.0),
            next_id: 1,
            downloads: HashMap::new(),
            bidi_ids: HashMap::new(),
            download_dir,
            kiosk,
            pid: page.pid(),
            scripts: Vec::new(),
        }
    }

    /// Take the next engine-local id.
    pub fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Move a finished download out of the Firefox download dir into the
    /// path the delegate chose. Reports the outcome as engine events.
    fn finish_download(&mut self, id: u64, out: &mut Vec<crate::EngineEvent>) {
        use crate::EngineEvent;
        let Some((_, dest)) = self.downloads.remove(&id) else {
            return;
        };
        let Some(dest) = dest else {
            out.push(EngineEvent::DownloadFailed {
                id,
                error: "cancelled".into(),
            });
            return;
        };
        // Firefox may suffix collisions (`name (1)`); take the newest match.
        std::thread::sleep(Duration::from_millis(200));
        let newest = std::fs::read_dir(&self.download_dir)
            .ok()
            .and_then(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_file())
                    .filter_map(|e| e.metadata().ok()?.modified().ok().map(|m| (m, e.path())))
                    .max_by_key(|(m, _)| *m)
            })
            .map(|(_, path)| path);
        match newest {
            Some(source) => {
                let moved = std::fs::rename(&source, &dest).or_else(|_| {
                    std::fs::copy(&source, &dest)
                        .map(|_| ())
                        .and_then(|()| std::fs::remove_file(&source))
                });
                match moved {
                    Ok(()) => out.push(EngineEvent::DownloadFinished { id }),
                    Err(e) => out.push(EngineEvent::DownloadFailed {
                        id,
                        error: format!("save failed: {e}"),
                    }),
                }
            }
            None => out.push(EngineEvent::DownloadFailed {
                id,
                error: "download file missing".into(),
            }),
        }
    }
}

/// Execute one engine command against a Firefox page, appending every
/// event it produced to `out`.
///
/// Shared by [`GeckoEngine`] and the `tontoo-webengine` helper renderer so
/// both speak the same protocol over the same BiDi session.
pub fn run_command(
    page: &GeckoPage,
    command: crate::EngineCommand,
    state: &mut GeckoState,
    out: &mut Vec<crate::EngineEvent>,
) {
    use crate::EngineCommand as C;
    use crate::EngineEvent as E;
    match command {
        C::LoadUrl(url) => {
            state.url = Some(url.clone());
            out.push(E::LoadStarted(Some(url.clone())));
            out.push(E::Url(Some(url.clone())));
            out.push(E::Progress(0.2));
            if let Err(e) = page.navigate(&url) {
                out.push(E::LoadFailed {
                    url: Some(url),
                    error: e,
                });
            }
        }
        C::LoadHtml { html, base_uri } => {
            state.url = None;
            out.push(E::LoadStarted(None));
            if let Err(e) = page.load_html(&html, base_uri.as_deref()) {
                out.push(E::LoadFailed { url: None, error: e });
            }
        }
        C::EvaluateJs { id, script } => match page.evaluate(&script) {
            Ok(result) => out.push(E::JsResult { id, result }),
            Err(e) => {
                eprintln!("gecko: evaluate failed ({e})");
                out.push(E::JsResult {
                    id,
                    result: JsonValue::Null,
                });
            }
        },
        C::Resize { width, height, scale } => {
            if let Err(e) = page.set_viewport(width, height, scale) {
                eprintln!("gecko: set_viewport failed ({e})");
            }
        }
        C::MouseDown { x, y } => {
            state.last_pos = (x, y);
            let _ = page.pointer_move(x, y);
            let _ = page.pointer_button(x, y, true);
        }
        C::MouseUp { x, y } => {
            state.last_pos = (x, y);
            let _ = page.pointer_move(x, y);
            let _ = page.pointer_button(x, y, false);
        }
        C::MouseMove { x, y } => {
            state.last_pos = (x, y);
            let _ = page.pointer_move(x, y);
        }
        C::Scroll { dx, dy } => {
            let (x, y) = state.last_pos;
            let _ = page.scroll(x, y, dx, dy);
        }
        C::KeyText(text) => {
            let _ = page.insert_text(&text);
        }
        C::SpecialKey { key } => {
            let _ = page.press_key(&key);
        }
        C::Reload => {
            let _ = page.reload(false);
        }
        C::ReloadBypassCache => {
            let _ = page.reload(true);
        }
        C::GoBack => {
            if let Err(e) = page.go_history(-1) {
                eprintln!("gecko: go_back failed ({e})");
            }
        }
        C::GoForward => {
            if let Err(e) = page.go_history(1) {
                eprintln!("gecko: go_forward failed ({e})");
            }
        }
        C::Stop => {
            let _ = page.stop();
        }
        C::DialogAnswer {
            confirmed,
            text,
            ..
        } => {
            // The dialog id only correlates the delegate round-trip; the
            // live prompt is answered straight away when it opens.
            let _ = (confirmed, text);
        }
        C::PermissionAnswer { .. } => {
            // Firefox shows its own permission prompt; the engine does not
            // pre-grant anything, so the default stays "ask the user".
        }
        C::DownloadDestination { id, path } => {
            if let Some((bidi, slot)) = state.downloads.get_mut(&id) {
                if path.is_none() {
                    let bidi = bidi.clone();
                    state.downloads.remove(&id);
                    state.bidi_ids.remove(&bidi);
                    out.push(E::DownloadFailed {
                        id,
                        error: "cancelled".into(),
                    });
                } else {
                    *slot = path;
                }
            }
        }
        C::CancelDownload { id } => {
            if let Some((bidi, _)) = state.downloads.remove(&id) {
                state.bidi_ids.remove(&bidi);
            }
        }
        C::ClearData { cookies, cache } => {
            if cookies || cache {
                if let Err(e) = page.clear_data() {
                    eprintln!("gecko: clear_data failed ({e})");
                }
            }
        }
        C::ListCookies { id } => match page.list_cookies(None) {
            Ok(cookies) => out.push(E::Cookies { id, cookies }),
            Err(e) => eprintln!("gecko: cookies failed ({e})"),
        },
        C::AddCookie { cookie } => {
            if let Err(e) = page.add_cookie(&cookie) {
                eprintln!("gecko: add_cookie failed ({e})");
            }
        }
        C::DeleteCookie { domain, path, name } => {
            if let Err(e) = page.delete_cookie(&domain, &path, &name) {
                eprintln!("gecko: delete_cookie failed ({e})");
            }
        }
        C::InstallExtension { id, path } => match page.install_extension(Path::new(&path)) {
            Ok(extension) => out.push(E::ExtensionInstalled { id, extension }),
            Err(error) => out.push(E::ExtensionFailed { id, error }),
        },
        C::ListExtensions { id } => match page.list_extensions() {
            Ok(extensions) => out.push(E::Extensions { id, extensions }),
            Err(e) => out.push(E::ExtensionFailed { id, error: e }),
        },
        C::NewTab { kind, .. } => match page.create_context(&kind) {
            Ok(context) => out.push(E::ContextReady { context }),
            Err(error) => eprintln!("gecko: new tab failed ({error})"),
        },
        C::CloseTab { context, .. } => match page.close_context(&context) {
            Ok(()) => out.push(E::ContextClosed { context }),
            Err(error) => eprintln!("gecko: close tab failed ({error})"),
        },
        C::ActivateTab { context, .. } => {
            if let Err(error) = page.activate_context(&context) {
                eprintln!("gecko: activate tab failed ({error})");
            }
        }
    }
}

/// Turn one BiDi event into engine protocol events.
pub fn handle_bidi_event(
    page: &GeckoPage,
    event: &BidiEvent,
    state: &mut GeckoState,
    out: &mut Vec<crate::EngineEvent>,
) {
    use crate::EngineEvent as E;
    match event {
        BidiEvent::ContextCreated { context } => {
            // A brand-new top-level window. The window of the initial
            // session was reported by `GeckoPage::launch` already.
            page.set_context(Some(context.clone()));
            state.url = None;
            out.push(E::LoadStarted(None));
            out.push(E::EngineWindow {
                pid: state.pid,
                kiosk: state.kiosk,
            });
            out.push(E::ReadyToShow);
        }        BidiEvent::NewContext { context } => {
            out.push(E::ContextReady {
                context: context.clone(),
            });
        }
        BidiEvent::ContextDestroyed { context } => {
            out.push(E::ContextClosed {
                context: context.clone(),
            });
            // Keep the session usable after the last window closes.
            if page.context().as_deref() == Some(context.as_str()) {
                let _ = page.refresh_context();
            }
        }
        BidiEvent::NavigationStarted { context, url } => {
            if page.context().as_deref() != Some(context.as_str()) {
                return;
            }
            if let Some(url) = url.clone() {
                state.url = Some(url.clone());
                out.push(E::Url(Some(url)));
            }
            out.push(E::Progress(0.1));
        }
        BidiEvent::DomContentLoaded { .. } => {
            out.push(E::Progress(0.6));
        }
        BidiEvent::Load { .. } => {
            let url = state.url.clone();
            if let Some(title) = page.title() {
                out.push(E::Title(Some(title)));
            }
            out.push(E::Progress(1.0));
            out.push(E::LoadFinished(url));
            if let Ok((can_back, can_forward)) = page.history_state() {
                out.push(E::History {
                    can_back,
                    can_forward,
                });
            }
            // User scripts and the message bridge run at document level.
            for script in state.scripts.clone() {
                if let Err(e) = page.inject(&script) {
                    eprintln!("gecko: user script failed ({e})");
                }
            }
            if let Err(e) = page.inject(MESSAGE_BOOTSTRAP) {
                eprintln!("gecko: message bridge failed ({e})");
            }
        }
        BidiEvent::UserPrompt {
            context,
            kind,
            message,
        } => {
            use crate::delegate::ScriptDialogKind;
            let id = state.take_id();
            let dialog = match kind.as_str() {
                "confirm" => ScriptDialogKind::Confirm,
                "prompt" => ScriptDialogKind::Prompt,
                "beforeunload" => ScriptDialogKind::BeforeUnloadConfirm,
                _ => ScriptDialogKind::Alert,
            };
            out.push(E::ScriptDialog {
                id,
                kind: dialog,
                message: message.clone(),
                prompt_default: None,
            });
            // Firefox blocks the content process until the prompt is
            // answered; answer the recorded default so the page proceeds.
            let _ = page.handle_user_prompt(context, true, None);
        }
        BidiEvent::DownloadBegin {
            download,
            url,
            filename,
        } => {
            let id = state.take_id();
            state
                .downloads
                .insert(id, (download.clone(), None));
            state.bidi_ids.insert(download.clone(), id);
            out.push(E::DownloadStarted {
                id,
                uri: url.clone(),
                suggested_filename: filename.clone(),
            });
        }
        BidiEvent::DownloadProgress {
            download,
            state: status,
            received,
        } => match status.as_str() {
            "completed" => {
                if let Some(id) = state.bidi_ids.get(download).copied() {
                    state.finish_download(id, out);
                }
            }
            "canceled" => {
                if let Some(id) = state.bidi_ids.remove(download) {
                    state.downloads.remove(&id);
                    out.push(E::DownloadFailed {
                        id,
                        error: "canceled".into(),
                    });
                }
            }
            _ => {
                if let Some(id) = state.bidi_ids.get(download).copied() {
                    out.push(E::DownloadProgress {
                        id,
                        progress: 0.0,
                        received_bytes: *received,
                    });
                }
            }
        },
        BidiEvent::ScriptMessage { data, .. } => {
            if let Some(name) = data.get("name").and_then(|n| n.as_str()) {
                out.push(E::ScriptMessage {
                    name: name.to_string(),
                    body: data.get("body").cloned().unwrap_or(JsonValue::Null),
                });
            }
        }
        BidiEvent::ConsoleLine { text } => {
            if let Some((name, body)) = parse_console_message(text) {
                out.push(E::ScriptMessage { name, body });
            }
        }
        BidiEvent::ExtensionInstalled { id } => {
            out.push(E::ExtensionInstalled {
                id: 0,
                extension: id.clone(),
            });
        }
    }
}

/// In-process [`crate::engine::WebEngine`] on top of a headful Firefox.
///
/// Unlike [`crate::transport::ProcessEngine`] this needs no helper binary:
/// the host process talks to Firefox over BiDi on two worker threads, one
/// draining commands and one draining engine events. Use it for browser
/// apps; use the helper when the engine must be isolated in its own
/// process or reached through the C ABI.
pub struct GeckoEngine {
    page: Arc<GeckoPage>,
    sender: SyncSender<crate::EngineCommand>,
    events: Arc<Mutex<Vec<crate::EngineEvent>>>,
}

impl GeckoEngine {
    /// Launch Firefox and wrap it in one step.
    pub fn launch(options: GeckoOptions) -> Result<Self, String> {
        let (page, receiver) = GeckoPage::launch(options.clone())?;
        Ok(Self::from_parts(page, receiver, options))
    }

    /// Wrap an already launched Firefox. `receiver` must be the event
    /// receiver [`GeckoPage::launch`] returned for that same session.
    pub fn from_parts(
        page: GeckoPage,
        receiver: Receiver<BidiEvent>,
        options: GeckoOptions,
    ) -> Self {
        Self::spawn(Arc::new(page), receiver, options)
    }

    fn spawn(
        page: Arc<GeckoPage>,
        receiver: Receiver<BidiEvent>,
        options: GeckoOptions,
    ) -> Self {
        // The first window exists before we can subscribe, so the engine
        // reports it here instead of waiting for `contextCreated`.
        let kiosk = options.kiosk;
        let events = Arc::new(Mutex::new(vec![
            crate::EngineEvent::EngineWindow {
                pid: page.pid(),
                kiosk,
            },
            crate::EngineEvent::ReadyToShow,
        ]));
        let state = Arc::new(Mutex::new(GeckoState::new(
            &page,
            options.download_dir.clone(),
            options.kiosk,
        )));

        // Event pump: BiDi events -> engine protocol events.
        let sink = Arc::clone(&events);
        let pump_page = Arc::clone(&page);
        let pump_state = Arc::clone(&state);
        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                let mut out = Vec::new();
                let mut state = pump_state.lock().expect("gecko state lock");
                handle_bidi_event(&pump_page, &event, &mut state, &mut out);
                drop(state);
                if !out.is_empty() {
                    sink.lock().expect("gecko events lock").extend(out);
                }
            }
        });

        // Command pump: engine commands -> BiDi calls.
        let (sender, receiver) = sync_channel::<crate::EngineCommand>(256);
        let sink = Arc::clone(&events);
        let cmd_page = Arc::clone(&page);
        let cmd_state = Arc::clone(&state);
        std::thread::spawn(move || {
            while let Ok(command) = receiver.recv() {
                let mut out = Vec::new();
                let mut state = cmd_state.lock().expect("gecko state lock");
                run_command(&cmd_page, command, &mut state, &mut out);
                drop(state);
                if !out.is_empty() {
                    sink.lock().expect("gecko events lock").extend(out);
                }
            }
        });

        Self {
            page,
            sender,
            events,
        }
    }

    /// The Firefox session behind this engine.
    pub fn page(&self) -> &Arc<GeckoPage> {
        &self.page
    }

    /// Firefox process id, for host window lookups.
    pub fn pid(&self) -> u32 {
        self.page.pid()
    }
}

impl crate::engine::WebEngine for GeckoEngine {
    fn send(&self, command: crate::EngineCommand) {
        let _ = self.sender.try_send(command);
    }

    /// Gecko draws into its own window, so there is no frame to hand out.
    fn latest_frame(&self) -> Option<Arc<crate::engine::SharedFrame>> {
        None
    }

    fn drain_events(&self) -> Vec<crate::EngineEvent> {
        std::mem::take(&mut *self.events.lock().expect("gecko events lock"))
    }

    /// In-process calls answer inline, so no event round-trip is needed.
    fn eval_sync(&self, script: &str) -> Option<JsonValue> {
        self.page.evaluate(script).ok()
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_values_use_the_webdriver_pua_table() {
        assert_eq!(bidi_key_value("Enter"), Some("\u{E007}"));
        assert_eq!(bidi_key_value("ArrowDown"), Some("\u{E015}"));
        assert_eq!(bidi_key_value("Nope"), None);
    }

    #[test]
    fn base64_matches_the_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn data_urls_carry_the_html() {
        let url = html_data_url("<h1>hi</h1>");
        assert!(url.starts_with("data:text/html;charset=utf-8;base64,"));
        assert!(url.ends_with(&base64_encode(b"<h1>hi</h1>")));
    }

    #[test]
    fn extension_policies_merge_into_the_profile() {
        let dir = std::env::temp_dir().join(format!(
            "tontoo-policy-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("profile dir");

        write_extension_policy(
            &dir,
            &[
                ExtensionPolicy::AddonId("uBlock0@raymondhill.net".into()),
                ExtensionPolicy::Url {
                    url: "https://addons.mozilla.org/firefox/downloads/latest/vimium/latest.xpi"
                        .into(),
                },
            ],
        )
        .expect("write policy");
        // Writing again must not drop the first entry.
        write_extension_policy(&dir, &[ExtensionPolicy::AddonId("jid1-MnnxcxisBPnSXQ@jetpack".into())])
            .expect("merge policy");

        let text = std::fs::read_to_string(dir.join("policies.json")).expect("policy file");
        let json = JsonValue::parse(&text).expect("policy json");
        let extensions = json
            .get("policies")
            .and_then(|p| p.get("3rdparty"))
            .and_then(|t| t.get("Extensions"))
            .expect("3rdparty.Extensions");
        let keys: Vec<&str> = extensions
            .object_entries()
            .map(|entries| entries.iter().map(|(k, _)| k.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(keys.len(), 3, "policy lost an entry: {text}");
        assert!(keys.contains(&"uBlock0@raymondhill.net"));
        assert!(keys.contains(&"jid1-MnnxcxisBPnSXQ@jetpack"));
        let ublock = extensions
            .get("uBlock0@raymondhill.net")
            .expect("ublock entry");
        assert_eq!(
            ublock.get("installation_mode").and_then(|m| m.as_str()),
            Some("normal_installed")
        );
        assert_eq!(
            extensions
                .get("https://addons.mozilla.org/firefox/downloads/latest/vimium/latest.xpi")
                .and_then(|e| e.get("URL"))
                .and_then(|u| u.as_str()),
            Some("https://addons.mozilla.org/firefox/downloads/latest/vimium/latest.xpi")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_policy_list_is_a_no_op() {
        let dir = std::env::temp_dir().join("tontoo-policy-empty-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("profile dir");
        write_extension_policy(&dir, &[]).expect("no-op");
        assert!(!dir.join("policies.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_lines_carry_nested_params() {
        // Regression: nested objects inside arrays must survive the
        // Foundation JSON writer, or Firefox answers "functionDeclaration
        // ... got Undefined".
        let params = JsonValue::Object(vec![
            (
                "functions".to_string(),
                JsonValue::Array(vec![JsonValue::Object(vec![(
                    "functionDeclaration".to_string(),
                    JsonValue::Str("1 + 1".to_string()),
                )])]),
            ),
            ("channel".to_string(), JsonValue::Str("tontoo".to_string())),
        ]);
        let line = method_call(7, "script.addPreloadScript", &params);
        let parsed = JsonValue::parse(&line).expect("command line is valid JSON");
        assert_eq!(parsed.get("id").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(
            parsed
                .get("params")
                .and_then(|p| p.get("functions"))
                .and_then(|f| f.as_array())
                .and_then(|f| f.first())
                .and_then(|f| f.get("functionDeclaration"))
                .and_then(|d| d.as_str()),
            Some("1 + 1"),
            "nested params were lost: {line}"
        );
        assert_eq!(
            parsed
                .get("params")
                .and_then(|p| p.get("channel"))
                .and_then(|c| c.as_str()),
            Some("tontoo")
        );
    }

    #[test]
    fn replies_and_events_split_on_the_id_field() {
        let reply = parse_message(r#"{"id":3,"type":"success","result":{"a":1}}"#);
        match reply {
            Incoming::Reply { id, result, error } => {
                assert_eq!(id, 3);
                assert!(result.is_some());
                assert!(error.is_none());
            }
            _ => panic!("expected a reply"),
        }
        let failure = parse_message(r#"{"id":4,"type":"error","error":"no such context"}"#);
        match failure {
            Incoming::Reply { id, error, .. } => {
                assert_eq!(id, 4);
                assert_eq!(error.as_deref(), Some("no such context"));
            }
            _ => panic!("expected an error reply"),
        }
        let event = parse_message(r#"{"method":"browsingContext.load","params":{"context":"c1"}}"#);
        assert!(matches!(event, Incoming::Event { .. }));
    }

    #[test]
    fn events_classify_by_method() {
        let params = |text: &str| JsonValue::parse(text).unwrap();
        let load = classify(
            "browsingContext.load",
            &params(r#"{"context":"abc","navigation":"n1"}"#),
        );
        assert!(matches!(load, Some(BidiEvent::Load { context }) if context == "abc"));
        let prompt = classify(
            "browsingContext.userPromptOpened",
            &params(r#"{"context":"abc","type":"confirm","message":"Sure?"}"#),
        );
        match prompt {
            Some(BidiEvent::UserPrompt { kind, message, .. }) => {
                assert_eq!(kind, "confirm");
                assert_eq!(message, "Sure?");
            }
            _ => panic!("expected a prompt"),
        }
        assert!(classify("network.responseCompleted", &params("{}")).is_none());
    }

    #[test]
    fn remote_objects_unwrap_to_plain_json() {
        let value = |text: &str| JsonValue::parse(text).unwrap();
        assert_eq!(
            remote_to_json(&value(r#"{"type":"number","value":2}"#)),
            JsonValue::Integer(2)
        );
        assert_eq!(
            remote_to_json(&value(r#"{"type":"undefined"}"#)),
            JsonValue::Null
        );
        let list = remote_to_json(&value(
            r#"{"type":"array","value":[{"type":"number","value":1},{"type":"string","value":"a"}]}"#,
        ));
        assert_eq!(
            list,
            JsonValue::Array(vec![JsonValue::Integer(1), JsonValue::Str("a".into())])
        );
        let map = remote_to_json(&value(
            r#"{"type":"map","value":{"a":{"type":"boolean","value":true}}}"#,
        ));
        assert_eq!(
            map,
            JsonValue::Object(vec![("a".to_string(), JsonValue::Bool(true))])
        );
        // Types without a JSON form collapse to null.
        assert_eq!(
            remote_to_json(&value(r#"{"type":"node","value":{"shared":[]}}"#)),
            JsonValue::Null
        );
        // Plain JSON passes through unchanged.
        assert_eq!(
            remote_to_json(&value(r#"{"a":1}"#)),
            JsonValue::Object(vec![("a".to_string(), JsonValue::Integer(1))])
        );
    }

    #[test]
    fn console_messages_carry_name_and_body() {
        let (name, body) =
            parse_console_message(r#"__tontoo__{"name":"navigator","body":{"a":1}}"#).unwrap();
        assert_eq!(name, "navigator");
        assert_eq!(body.get("a").and_then(|v| v.as_u64()), Some(1));
        assert!(parse_console_message("just a console line").is_none());
    }

    #[test]
    fn launch_arguments_carry_the_remote_port() {
        let profile = Path::new("/tmp/profile");
        let args = firefox_args(
            profile,
            4444,
            true,
            false,
            false,
            Some((1200, 800)),
            Some("https://x.test"),
        );
        assert!(args.contains(&"--remote-debugging-port".to_string()));
        assert!(args.iter().any(|a| a == "--window-size=1200,800"));
        assert!(args.contains(&"--kiosk".to_string()));
        assert_eq!(args.last().unwrap(), "https://x.test");
        let headless =
            firefox_args(profile, 1, true, true, true, None, None);
        assert!(headless.contains(&"--headless".to_string()));
        assert!(!headless.contains(&"--kiosk".to_string()));
        assert!(headless.contains(&"--private-window".to_string()));
        assert!(!headless.iter().any(|a| a.starts_with("--window-size")));
        assert_eq!(headless.last().unwrap(), "about:blank");
        // Kiosk is opt-in: it is a fullscreen request, and compositors
        // that answer with a 0 x 0 size abort Firefox.
        let default_args = firefox_args(profile, 1, false, false, false, None, None);
        assert!(!default_args.contains(&"--kiosk".to_string()));
    }

    #[test]
    fn prefs_enable_bidi_and_extensions() {
        let prefs = profile_prefs(1234, Path::new("/downloads"), false);
        let find = |key: &str| {
            prefs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(find("remote.active-protocols"), "3");
        assert_eq!(find("remote.debugging.port"), "1234");
        assert_eq!(find("xpinstall.signatures.required"), "false");
        assert_eq!(find("extensions.autoDisableScopes"), "0");
        assert_eq!(find("app.update.auto"), "false");
        assert_eq!(find("browser.download.dir"), "/downloads");
    }
}
