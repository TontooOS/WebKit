//! Headless-Chromium driver over the DevTools protocol.
//!
//! Spawns `chromium --headless=new` with the sandbox left ON and drives one
//! page target: navigation, history, screenshots (BGRA frames), input, real
//! JavaScript results, cookies, downloads and dialogs. Page JavaScript runs
//! inside the real Chromium sandbox, never in our process.
//!
//! The helper (`tontoo-webengine`) uses this when a Chromium binary is
//! present and falls back to the mock renderer otherwise. Permissions fail
//! closed: Chromium auto-denies prompts unless pre-granted, and this driver
//! never pre-grants, matching the deny-by-default delegate.

#![cfg(feature = "chromium")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc::{SyncSender, Receiver, sync_channel},
};
use std::time::{Duration, Instant};

use tungstenite::{Message, WebSocket, connect};

use crate::cookie::Cookie;

/// How long a single CDP round-trip may take (cold browsers in
/// containers answer slowly; the fontconfig scan alone can take 10 s).
pub const CDP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for the DevTools endpoint at launch.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(25);

/// Locate a Chromium binary: `CHROMIUM_BIN`/`CHROME_BIN`, the managed
/// self-provisioned build, then well-known names on `PATH` (smoke-tested,
/// so a broken system build is skipped). As a last resort (auto-update
/// enabled) a build is downloaded.
pub fn find_chrome() -> Result<PathBuf, String> {
    for var in ["CHROMIUM_BIN", "CHROME_BIN"] {
        if let Ok(path) = std::env::var(var) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    if let Some((_, bin)) = super::chrome_provision::installed() {
        return Ok(bin);
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "chromium.exe",
        "chrome.exe",
    ] {
        if let Some(path) = super::transport::find_on_path(name) {
            if super::chrome_provision::smoke_test(&path) {
                return Ok(path);
            }
            eprintln!("tontoo-webengine: system {name} does not run, skipping");
        }
    }
    super::chrome_provision::ensure_chrome()
}

/// Raw CDP event (method + params) coming off a connection.
pub struct CdpRawEvent {
    pub method: String,
    pub params: serde_json::Value,
}

/// Helper-facing CDP events after classification.
#[derive(Debug, Clone)]
pub enum CdpEvent {
    Started {
        url: String,
    },
    Navigated {
        frame_id: String,
        url: String,
    },
    Loaded,
    Dialog {
        dialog_type: String,
        message: String,
        default_prompt: String,
    },
    DownloadBegin {
        guid: String,
        url: String,
        filename: String,
    },
    DownloadProgress {
        guid: String,
        received: u64,
        state: String,
    },
}

/// One CDP connection (page target or browser endpoint).
pub struct CdpConn {
    socket: Mutex<WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>>,
    next_id: AtomicU32,
    pending: Mutex<HashMap<u32, SyncSender<Result<serde_json::Value, String>>>>,
    shutdown: AtomicBool,
}

impl CdpConn {
    /// Connect and pump incoming messages: replies route to `call`, events
    /// go to `events`. Runs until shutdown or disconnect.
    pub fn connect(url: &str, events: SyncSender<CdpRawEvent>) -> Result<Arc<Self>, String> {
        let (mut socket, _) =
            connect(url).map_err(|e| format!("websocket connect: {e}"))?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        }
        let conn = Arc::new(Self {
            socket: Mutex::new(socket),
            next_id: AtomicU32::new(1),
            pending: Mutex::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
        });
        let worker = Arc::clone(&conn);
        let heartbeat = std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some();
        let mut timeouts = 0u32;
        std::thread::spawn(move || {
            loop {
                if worker.shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let text = {
                    let mut socket = worker.socket.lock().expect("cdp socket lock");
                    match socket.read() {
                        Ok(Message::Text(text)) => Some(text.to_string()),
                        Ok(_) => continue,
                        Err(tungstenite::Error::Io(e))
                            if e.kind() == std::io::ErrorKind::TimedOut
                                || e.kind() == std::io::ErrorKind::WouldBlock =>
                        {
                            timeouts += 1;
                            if heartbeat && timeouts % 40 == 1 {
                                eprintln!("debug: cdp reader alive (timeouts={timeouts})");
                            }
                            continue
                        }
                        Err(_) => break,
                    }
                };
                let Some(text) = text else { continue };
                match parse_message(&text) {
                    Incoming::Reply { id, result, error } => {
                        if let Some(tx) = worker.pending.lock().expect("pending lock").remove(&id)
                        {
                            let _ = tx.try_send(match error {
                                Some(e) => Err(e),
                                None => Ok(result.unwrap_or(serde_json::Value::Null)),
                            });
                        }
                    }
                    Incoming::Event { method, params } => {
                        let _ = events.try_send(CdpRawEvent { method, params });
                    }
                    Incoming::Other => {}
                }
            }
        });
        Ok(conn)
    }

    /// Send a method call and wait for its result.
    pub fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = sync_channel(1);
        self.pending.lock().expect("pending lock").insert(id, tx);
        let line = method_call(id, method, &params);
        let debug = std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some();
        if debug {
            eprintln!("debug: cdp send {method} id={id}");
        }
        {
            let mut socket = self.socket.lock().expect("cdp socket lock");
            socket
                .send(Message::Text(line.into()))
                .map_err(|e| format!("websocket send: {e}"))?;
        }
        if debug {
            eprintln!("debug: cdp sent {method} id={id}, waiting");
        }
        let deadline = Instant::now() + CDP_TIMEOUT;
        loop {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("cdp connection closed".into())
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                self.pending.lock().expect("pending lock").remove(&id);
                return Err(format!("cdp call timed out: {method}"));
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
            if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
    }
}

/// Classified inbound CDP message.
pub enum Incoming {
    Reply {
        id: u32,
        result: Option<serde_json::Value>,
        error: Option<String>,
    },
    Event {
        method: String,
        params: serde_json::Value,
    },
    Other,
}

/// Parse one inbound CDP text message.
pub(crate) fn parse_message(text: &str) -> Incoming {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Incoming::Other,
    };
    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
        if value.get("method").is_some() {
            return Incoming::Other;
        }
        let error = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_string);
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

/// Build a CDP method call line.
pub(crate) fn method_call(id: u32, method: &str, params: &serde_json::Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

/// Map a Runtime RemoteObject to plain JSON. Non-serializable numbers
/// (`NaN`, infinities) and missing values become null.
pub(crate) fn remote_to_json(obj: &serde_json::Value) -> serde_json::Value {
    if obj.get("type").and_then(|t| t.as_str()) == Some("undefined") {
        return serde_json::Value::Null;
    }
    if let Some(unserializable) = obj.get("unserializableValue").and_then(|v| v.as_str()) {
        return match unserializable {
            "0" | "-0" => serde_json::json!(0),
            _ => serde_json::Value::Null,
        };
    }
    obj.get("value").cloned().unwrap_or(serde_json::Value::Null)
}

/// Decode a PNG screenshot to BGRA bytes plus dimensions.
pub(crate) fn png_to_bgra(png: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let decoded = image::load_from_memory(png).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut bgra = decoded.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some((bgra, width, height))
}

/// Pick the page target WebSocket URL from `/json/list`.
pub(crate) fn pick_page_target(list: &serde_json::Value) -> Option<String> {
    list.as_array()?.iter().find_map(|target| {
        if target.get("type").and_then(|t| t.as_str()) == Some("page") {
            target
                .get("webSocketDebuggerUrl")
                .and_then(|u| u.as_str())
                .map(str::to_string)
        } else {
            None
        }
    })
}

/// Extract the browser WebSocket URL from a DevTools stderr line.
pub(crate) fn devtools_ws_from_line(line: &str) -> Option<String> {
    let marker = "DevTools listening on ";
    let start = line.find(marker)? + marker.len();
    Some(line[start..].trim().to_string())
}

/// Minimal HTTP GET over plain TCP (no extra deps for `/json/*`).
/// HTTP/1.1 is required: the DevTools server ignores HTTP/1.0 requests.
/// The server keeps the connection open, so exactly `Content-Length` body
/// bytes are read instead of waiting for EOF.
fn http_get_json(host: &str, port: u16, path: &str) -> Result<serde_json::Value, String> {
    let mut stream = TcpStream::connect((host, port))
        .map_err(|e| format!("devtools http: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("devtools http: {e}"))?;
    // Read until headers + Content-Length body bytes arrived.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let body: Vec<u8> = loop {
        match stream.read(&mut chunk) {
            Ok(0) => break None,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if let Some(header_end) = find_header_end(&raw) {
                    let length = content_length(&raw[..header_end]);
                    if raw.len() >= header_end + length {
                        break Some(raw[header_end..header_end + length].to_vec());
                    }
                }
                if raw.len() > 4 << 20 {
                    break None;
                }
            }
            Err(_) => break None,
        }
    }
    .ok_or_else(|| "devtools http: incomplete response".to_string())?;
    serde_json::from_slice(&body).map_err(|e| format!("devtools json: {e}"))
}

/// Position just past the header terminator, if fully received.
fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// Parse the response `Content-Length` (0 when absent).
fn content_length(header: &[u8]) -> usize {
    let text = String::from_utf8_lossy(header).to_lowercase();
    text.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })
    .unwrap_or(0)
}

/// Pick an unused loopback TCP port.
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("free port: {e}"))?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

/// Extract `(host, port)` from a `ws://host:port/...` DevTools URL.
/// Falls back to 127.0.0.1 and the launch port when unparseable.
pub(crate) fn ws_authority(url: &str, fallback_port: u16) -> (String, u16) {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or("");
    let mut parts = authority.rsplitn(2, ':');
    let port = parts
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(fallback_port);
    let host = parts.next().unwrap_or("127.0.0.1");
    (host.to_string(), port)
}

/// Chromium launch flags. The sandbox stays ON; `no_sandbox` is only for
/// containers without user namespaces (`TONTOO_CHROME_NO_SANDBOX=1`).
pub(crate) fn chrome_args(port: u16, profile: &std::path::Path, no_sandbox: bool) -> Vec<String> {
    let mut args = vec![
        "--headless=new".to_string(),
        format!("--remote-debugging-port={port}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-sync".to_string(),
        "--disable-features=Translate,TranslateUI".to_string(),
        "--hide-scrollbars".to_string(),
        "--disable-dev-shm-usage".to_string(),
        "--enable-unsafe-swiftshader".to_string(),
        // No background network: component updates and friends stall
        // CDP responses on first run (and leak beyond the page).
        "--disable-component-update".to_string(),
        "--disable-background-networking".to_string(),
        format!("--user-data-dir={}", profile.to_string_lossy()),
        "about:blank".to_string(),
    ];
    if no_sandbox {
        args.push("--no-sandbox".to_string());
    }
    args
}

/// One driven Chromium page: owns the browser child, both CDP connections
/// and the profile dir. Classified helper-facing events arrive on the
/// receiver returned by [`CdpPage::launch`].
pub struct CdpPage {
    child: Mutex<Child>,
    profile: PathBuf,
    page: Arc<CdpConn>,
    browser: Arc<CdpConn>,
    frame_id: Mutex<String>,
    viewport: Mutex<(u32, u32, f32)>,
}

impl CdpPage {
    /// Launch Chromium and attach to its first page target.
    pub fn launch(download_dir: PathBuf) -> Result<(Self, Receiver<CdpEvent>), String> {
        let chrome = find_chrome()?;
        let port = free_port()?;
        let profile = std::env::temp_dir().join(format!(
            "tontoo-chrome-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
        let no_sandbox = std::env::var_os("TONTOO_CHROME_NO_SANDBOX").is_some();
        if no_sandbox {
            eprintln!("chromium: sandbox DISABLED via TONTOO_CHROME_NO_SANDBOX");
        }
        let mut child = Command::new(&chrome)
            .args(chrome_args(port, &profile, no_sandbox))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn chromium: {e}"))?;

        // Wait for the DevTools endpoint on stderr.
        let stderr = child.stderr.take().ok_or("chromium stderr unavailable")?;
        let (ws_tx, ws_rx) = sync_channel::<String>(1);
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines().flatten() {
                if let Some(url) = devtools_ws_from_line(&line) {
                    let _ = ws_tx.try_send(url);
                    break;
                }
            }
        });
        let browser_url = ws_rx.recv_timeout(LAUNCH_TIMEOUT).map_err(|e| {
            use std::sync::mpsc::RecvTimeoutError;
            match e {
                // Child died before printing the endpoint (broken binary).
                RecvTimeoutError::Disconnected => {
                    "chromium exited during startup (broken binary?)".to_string()
                }
                RecvTimeoutError::Timeout => {
                    "timed out waiting for DevTools endpoint".to_string()
                }
            }
        })?;
        // Talk HTTP to the same authority the browser WebSocket proved.
        let (http_host, http_port) = ws_authority(&browser_url, port);

        let (raw_tx, raw_rx) = sync_channel::<CdpRawEvent>(256);
        let browser =
            CdpConn::connect(&browser_url, raw_tx.clone()).map_err(|e| format!("browser: {e}"))?;

        // Find the page target, polling briefly: it registers a moment
        // after the DevTools endpoint appears. Create one as fallback.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut page_url = None;
        while page_url.is_none() && Instant::now() < deadline {
            match http_get_json(&http_host, http_port, "/json/list") {
                Ok(list) => page_url = pick_page_target(&list),
                Err(e) => {
                    if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
                        eprintln!("debug: /json/list failed: {e}");
                    }
                }
            }
            if page_url.is_none() {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        if page_url.is_none() {
            if let Ok(created) = http_get_json(&http_host, http_port, "/json/new?about:blank") {
                page_url = created
                    .get("webSocketDebuggerUrl")
                    .and_then(|u| u.as_str())
                    .map(str::to_string);
            }
        }
        let page_url = page_url.ok_or("no page target in chromium")?;
        let page = CdpConn::connect(&page_url, raw_tx).map_err(|e| format!("page: {e}"))?;

        let (classified_tx, classified_rx) = sync_channel::<CdpEvent>(256);
        classify_thread(raw_rx, classified_tx);

        let driven = Self {
            child: Mutex::new(child),
            profile,
            page,
            browser,
            frame_id: Mutex::new(String::new()),
            viewport: Mutex::new((800, 600, 1.0)),
        };
        driven.setup(&download_dir)?;
        Ok((driven, classified_rx))
    }

    /// Post-connect initialization, with per-step debug markers.
    fn setup(&self, download_dir: &std::path::Path) -> Result<(), String> {
        self.step("Page.enable", || {
            self.page.call("Page.enable", serde_json::json!({}))
        })?;
        self.step("Runtime.enable", || {
            self.page.call("Runtime.enable", serde_json::json!({}))
        })?;
        self.step("Network.enable", || {
            self.page.call("Network.enable", serde_json::json!({}))
        })?;
        self.step("Page.setDownloadBehavior", || {
            self.page.call(
                "Page.setDownloadBehavior",
                serde_json::json!({
                    "behavior": "allow",
                    "downloadPath": download_dir.to_string_lossy(),
                }),
            )
        })?;
        if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
            eprintln!("debug: cdp setup Page.getFrameTree ...");
        }
        if let Ok(tree) = self
            .page
            .call("Page.getFrameTree", serde_json::json!({}))
        {
            if let Some(id) = tree
                .get("frameTree")
                .and_then(|t| t.get("frame"))
                .and_then(|f| f.get("id"))
                .and_then(|i| i.as_str())
            {
                *self.frame_id.lock().expect("frame lock") = id.to_string();
            }
            if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
                eprintln!("debug: cdp setup Page.getFrameTree -> true");
            }
        } else if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
            eprintln!("debug: cdp setup Page.getFrameTree -> false");
        }
        self.step("Emulation.setDeviceMetricsOverride", || {
            self.apply_viewport().map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    fn step(
        &self,
        name: &str,
        call: impl FnOnce() -> Result<serde_json::Value, String>,
    ) -> Result<(), String> {
        if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
            eprintln!("debug: cdp setup {name} ...");
        }
        let result = call();
        if std::env::var_os("TONTOO_WEBENGINE_DEBUG").is_some() {
            eprintln!("debug: cdp setup {name} -> {}", result.is_ok());
        }
        result.map(|_| ())
    }

    fn apply_viewport(&self) -> Result<(), String> {
        let (w, h, scale) = *self.viewport.lock().expect("viewport lock");
        self.page.call(
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({
                "width": w, "height": h,
                "deviceScaleFactor": scale, "mobile": false,
            }),
        )?;
        Ok(())
    }

    /// Resize the viewport (physical pixels + scale).
    pub fn set_viewport(&self, width: u32, height: u32, scale: f32) -> Result<(), String> {
        *self.viewport.lock().expect("viewport lock") = (width.max(1), height.max(1), scale);
        self.apply_viewport()
    }

    /// Navigate the main frame.
    pub fn navigate(&self, url: &str) -> Result<(), String> {
        self.page
            .call("Page.navigate", serde_json::json!({ "url": url }))?;
        Ok(())
    }

    /// Load raw HTML into the main frame.
    pub fn set_document_content(&self, html: &str) -> Result<(), String> {
        let frame_id = self.frame_id.lock().expect("frame lock").clone();
        self.page.call(
            "Page.setDocumentContent",
            serde_json::json!({ "frameId": frame_id, "html": html }),
        )?;
        Ok(())
    }

    /// Reload, optionally bypassing cache.
    pub fn reload(&self, bypass_cache: bool) -> Result<(), String> {
        self.page.call(
            "Page.reload",
            serde_json::json!({ "ignoreCache": bypass_cache }),
        )?;
        Ok(())
    }

    /// Stop the current load.
    pub fn stop_loading(&self) -> Result<(), String> {
        self.page.call("Page.stopLoading", serde_json::json!({}))?;
        Ok(())
    }

    /// Session history state.
    pub fn history_state(&self) -> Result<(bool, bool), String> {
        let history = self
            .page
            .call("Page.getNavigationHistory", serde_json::json!({}))?;
        let index = history.get("currentIndex").and_then(|i| i.as_u64()).unwrap_or(0);
        let len = history
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|a| a.len() as u64)
            .unwrap_or(0);
        Ok((index > 0, index + 1 < len))
    }

    /// Step through session history. Returns false at the ends.
    pub fn go_history(&self, delta: i64) -> Result<bool, String> {
        let history = self
            .page
            .call("Page.getNavigationHistory", serde_json::json!({}))?;
        let index = history.get("currentIndex").and_then(|i| i.as_i64()).unwrap_or(0);
        let entries = history
            .get("entries")
            .and_then(|e| e.as_array())
            .cloned()
            .unwrap_or_default();
        let target = index + delta;
        if target < 0 || target >= entries.len() as i64 {
            return Ok(false);
        }
        let id = entries[target as usize].get("id").cloned().unwrap_or_default();
        self.page.call(
            "Page.navigateToHistoryEntry",
            serde_json::json!({ "entryId": id }),
        )?;
        Ok(true)
    }

    /// Screenshot the viewport and return BGRA bytes plus size.
    pub fn capture(&self) -> Result<(Vec<u8>, u32, u32), String> {
        let shot = self.page.call(
            "Page.captureScreenshot",
            serde_json::json!({ "format": "png" }),
        )?;
        let data = shot
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or("screenshot without data")?;
        let png = base64_decode(data).ok_or("screenshot base64")?;
        png_to_bgra(&png).ok_or_else(|| "screenshot decode failed".to_string())
    }

    /// Evaluate JavaScript and return the JSON value.
    pub fn evaluate(&self, script: &str) -> Result<serde_json::Value, String> {
        let reply = self.page.call(
            "Runtime.evaluate",
            serde_json::json!({
                "expression": script,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )?;
        if let Some(exception) = reply.get("exceptionDetails") {
            let text = exception
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("javascript exception");
            return Err(text.to_string());
        }
        Ok(reply
            .get("result")
            .map(remote_to_json)
            .unwrap_or(serde_json::Value::Null))
    }

    /// Document title via JS.
    pub fn title(&self) -> Option<String> {
        self.evaluate("document.title")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
    }

    /// Pointer move in CSS px.
    pub fn mouse_move(&self, x: f64, y: f64) -> Result<(), String> {
        self.page.call(
            "Input.dispatchMouseEvent",
            serde_json::json!({ "type": "mouseMoved", "x": x, "y": y }),
        )?;
        Ok(())
    }

    /// Mouse press/release in CSS px.
    pub fn mouse_button(&self, x: f64, y: f64, pressed: bool) -> Result<(), String> {
        self.page.call(
            "Input.dispatchMouseEvent",
            serde_json::json!({
                "type": if pressed { "mousePressed" } else { "mouseReleased" },
                "x": x, "y": y, "button": "left", "clickCount": 1,
            }),
        )?;
        Ok(())
    }

    /// Scroll wheel delta in CSS px.
    pub fn wheel(&self, x: f64, y: f64, dx: f64, dy: f64) -> Result<(), String> {
        self.page.call(
            "Input.dispatchMouseEvent",
            serde_json::json!({
                "type": "mouseWheel", "x": x, "y": y,
                "deltaX": dx, "deltaY": dy,
            }),
        )?;
        Ok(())
    }

    /// Insert printable text.
    pub fn insert_text(&self, text: &str) -> Result<(), String> {
        self.page.call(
            "Input.insertText",
            serde_json::json!({ "text": text }),
        )?;
        Ok(())
    }

    /// Press a non-printable key (Enter, Backspace, arrows, Escape).
    pub fn special_key(&self, key: &str) -> Result<(), String> {
        let code = match key {
            "Enter" => 13,
            "Backspace" => 8,
            "Escape" => 27,
            "ArrowLeft" => 37,
            "ArrowUp" => 38,
            "ArrowRight" => 39,
            "ArrowDown" => 40,
            _ => return Ok(()),
        };
        self.page.call(
            "Input.dispatchKeyEvent",
            serde_json::json!({
                "type": "rawKeyDown", "key": key, "code": key,
                "windowsVirtualKeyCode": code,
            }),
        )?;
        Ok(())
    }

    /// Answer a JavaScript dialog.
    pub fn dialog_answer(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        let mut params = serde_json::json!({ "accept": accept });
        if let Some(text) = text {
            params["promptText"] = serde_json::Value::String(text.to_string());
        }
        self.page.call("Page.handleJavaScriptDialog", params)?;
        Ok(())
    }

    /// Cancel a download by guid.
    pub fn cancel_download(&self, guid: &str) -> Result<(), String> {
        self.browser.call(
            "Browser.cancelDownload",
            serde_json::json!({ "guid": guid }),
        )?;
        Ok(())
    }

    /// List cookies of the store.
    pub fn list_cookies(&self) -> Result<Vec<Cookie>, String> {
        let reply = self.page.call("Storage.getCookies", serde_json::json!({}))?;
        let mut out = Vec::new();
        if let Some(items) = reply.get("cookies").and_then(|c| c.as_array()) {
            for item in items {
                let mut cookie = Cookie::new(
                    item.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                    item.get("value").and_then(|v| v.as_str()).unwrap_or_default(),
                    item.get("domain").and_then(|v| v.as_str()).unwrap_or_default(),
                );
                if let Some(path) = item.get("path").and_then(|v| v.as_str()) {
                    cookie = cookie.path(path);
                }
                cookie = cookie
                    .secure(item.get("secure").and_then(|v| v.as_bool()).unwrap_or(false))
                    .http_only(item.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false));
                out.push(cookie);
            }
        }
        Ok(out)
    }

    /// Add or update a cookie.
    pub fn add_cookie(&self, cookie: &Cookie) -> Result<(), String> {
        self.page.call(
            "Storage.setCookies",
            serde_json::json!({ "cookies": [{
                "name": cookie.name, "value": cookie.value,
                "domain": cookie.domain, "path": cookie.path,
                "secure": cookie.secure, "httpOnly": cookie.http_only,
            }] }),
        )?;
        Ok(())
    }

    /// Delete one cookie.
    pub fn delete_cookie(&self, domain: &str, path: &str, name: &str) -> Result<(), String> {
        self.page.call(
            "Storage.deleteCookies",
            serde_json::json!({ "name": name, "domain": domain, "path": path }),
        )?;
        Ok(())
    }

    /// Clear cookies and/or cache.
    pub fn clear_data(&self, cookies: bool, cache: bool, origin: Option<&str>) -> Result<(), String> {
        if cookies {
            self.page.call("Storage.clearCookies", serde_json::json!({}))?;
        }
        if cache {
            self.page.call("Network.clearBrowserCache", serde_json::json!({}))?;
            if let Some(origin) = origin {
                let _ = self.page.call(
                    "Storage.clearDataForOrigin",
                    serde_json::json!({ "origin": origin, "storageTypes": "all" }),
                );
            }
        }
        Ok(())
    }
}

impl Drop for CdpPage {
    fn drop(&mut self) {
        self.page.close();
        self.browser.close();
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// Classify raw CDP events into helper-facing events on a worker thread.
fn classify_thread(raw: Receiver<CdpRawEvent>, out: SyncSender<CdpEvent>) {
    std::thread::spawn(move || {
        for event in raw {
            let classified = match event.method.as_str() {
                "Page.frameStartedLoading" => event
                    .params
                    .get("url")
                    .and_then(|u| u.as_str())
                    .map(|url| CdpEvent::Started {
                        url: url.to_string(),
                    }),
                "Page.frameNavigated" => {
                    let frame = event.params.get("frame");
                    let url = frame
                        .and_then(|f| f.get("url"))
                        .and_then(|u| u.as_str())
                        .unwrap_or_default();
                    let id = frame
                        .and_then(|f| f.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or_default();
                    // Loader-only navigations carry no id; skip those.
                    if id.is_empty() {
                        None
                    } else {
                        Some(CdpEvent::Navigated {
                            frame_id: id.to_string(),
                            url: url.to_string(),
                        })
                    }
                }
                "Page.loadEventFired" => Some(CdpEvent::Loaded),
                "Page.javascriptDialogOpening" => Some(CdpEvent::Dialog {
                    dialog_type: event
                        .params
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("alert")
                        .to_string(),
                    message: event
                        .params
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    default_prompt: event
                        .params
                        .get("defaultPrompt")
                        .and_then(|p| p.as_str())
                        .unwrap_or_default()
                        .to_string(),
                }),
                "Browser.downloadWillBegin" => Some(CdpEvent::DownloadBegin {
                    guid: event
                        .params
                        .get("guid")
                        .and_then(|g| g.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: event
                        .params
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    filename: event
                        .params
                        .get("suggestedFilename")
                        .and_then(|f| f.as_str())
                        .unwrap_or("download")
                        .to_string(),
                }),
                "Browser.downloadProgress" => Some(CdpEvent::DownloadProgress {
                    guid: event
                        .params
                        .get("guid")
                        .and_then(|g| g.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    received: event
                        .params
                        .get("receivedBytes")
                        .and_then(|r| r.as_u64())
                        .unwrap_or(0),
                    state: event
                        .params
                        .get("state")
                        .and_then(|s| s.as_str())
                        .unwrap_or("inProgress")
                        .to_string(),
                }),
                _ => None,
            };
            if let Some(event) = classified {
                let _ = out.try_send(event);
            }
        }
    });
}

/// Minimal base64 decoder (screenshots only, no extra deps).
pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 256] = &{
        let mut table = [255u8; 256];
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < alphabet.len() {
            table[alphabet[i] as usize] = i as u8;
            i += 1;
        }
        table
    };
    let clean: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks_exact(4) {
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                pad += 1;
                n <<= 6;
            } else {
                let v = TABLE[b as usize];
                if v == 255 {
                    return None;
                }
                n = (n << 6) | v as u32;
                let _ = i;
            }
        }
        // Padding only valid at the end; keep the decoder strict enough
        // for chrome output without over-validating.
        if pad > 2 {
            return None;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_line_shape() {
        let line = method_call(3, "Page.navigate", &serde_json::json!({ "url": "https://x.test" }));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 3);
        assert_eq!(value["method"], "Page.navigate");
    }

    #[test]
    fn reply_and_event_routing() {
        match parse_message(r#"{"id":7,"result":{"frameId":"A"}}"#) {
            Incoming::Reply { id: 7, .. } => {}
            _ => panic!("expected reply"),
        }
        match parse_message(r#"{"id":8,"error":{"code":-32000,"message":"nope"}}"#) {
            Incoming::Reply { error: Some(_), .. } => {}
            _ => panic!("expected error reply"),
        }
        match parse_message(r#"{"method":"Page.loadEventFired","params":{}}"#) {
            Incoming::Event { method, .. } if method == "Page.loadEventFired" => {}
            _ => panic!("expected event"),
        }
        assert!(matches!(parse_message("garbage"), Incoming::Other));
    }

    #[test]
    fn remote_objects_map_to_json() {
        let v = remote_to_json(&serde_json::json!({ "type": "string", "value": "hi" }));
        assert_eq!(v, serde_json::json!("hi"));
        let v = remote_to_json(&serde_json::json!({ "type": "undefined" }));
        assert_eq!(v, serde_json::Value::Null);
        let v = remote_to_json(&serde_json::json!({ "type": "number", "unserializableValue": "NaN" }));
        assert_eq!(v, serde_json::Value::Null);
        let v = remote_to_json(&serde_json::json!({ "type": "object", "subtype": "null", "value": null }));
        assert_eq!(v, serde_json::Value::Null);
    }

    #[test]
    fn target_picking() {
        let list = serde_json::json!([
            { "type": "browser", "webSocketDebuggerUrl": "ws://b" },
            { "type": "page", "webSocketDebuggerUrl": "ws://p" },
        ]);
        assert_eq!(pick_page_target(&list).as_deref(), Some("ws://p"));
        assert!(pick_page_target(&serde_json::json!([])).is_none());
    }

    #[test]
    fn devtools_line_parsing() {
        let url = devtools_ws_from_line("DevTools listening on ws://127.0.0.1:9222/devtools/browser/abc");
        assert_eq!(url.as_deref(), Some("ws://127.0.0.1:9222/devtools/browser/abc"));
        assert!(devtools_ws_from_line("[INFO] ready").is_none());
    }

    #[test]
    fn launch_flags_keep_sandbox() {
        let args = chrome_args(9222, std::path::Path::new("/tmp/p"), false);
        assert!(args.iter().any(|a| a == "--headless=new"));
        assert!(args.iter().any(|a| a == "--remote-debugging-port=9222"));
        assert!(!args.iter().any(|a| a == "--no-sandbox"));
        let args = chrome_args(9222, std::path::Path::new("/tmp/p"), true);
        assert!(args.iter().any(|a| a == "--no-sandbox"));
    }

    #[test]
    fn base64_roundtrip() {
        // "Hello" in base64, plus padding variants.
        assert_eq!(base64_decode("SGVsbG8=").as_deref(), Some(b"Hello".as_slice()));
        assert_eq!(base64_decode("SGk=").as_deref(), Some(b"Hi".as_slice()));
        assert_eq!(base64_decode("TQ==").as_deref(), Some(b"M".as_slice()));
        assert!(base64_decode("!!!").is_none());
        assert!(base64_decode("ABC").is_none());
    }

    #[test]
    fn png_decodes_to_bgra() {
        // 2x1 PNG: red, green. Encoded by hand (raw RGB + zlib via image).
        let mut png = Vec::new();
        {
            let img = image::RgbImage::from_raw(2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap();
            let encoder = image::codecs::png::PngEncoder::new(&mut png);
            use image::ImageEncoder;
            encoder
                .write_image(&img.into_raw(), 2, 1, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        let (bgra, w, h) = png_to_bgra(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(&bgra[0..4], &[0, 0, 255, 255]); // red -> BGRA
        assert_eq!(&bgra[4..8], &[0, 255, 0, 255]); // green stays
    }

    #[test]
    fn missing_chrome_is_an_error() {
        std::env::remove_var("CHROMIUM_BIN");
        std::env::remove_var("CHROME_BIN");
        // Passes trivially when no chrome exists; exercises the lookup.
        let _ = find_chrome();
    }

    #[test]
    fn ws_authority_parsing() {
        assert_eq!(
            ws_authority("ws://127.0.0.1:9222/devtools/browser/abc", 1),
            ("127.0.0.1".to_string(), 9222)
        );
        assert_eq!(
            ws_authority("garbage", 1234),
            ("127.0.0.1".to_string(), 1234)
        );
    }

    #[test]
    fn header_helpers() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 363\r\n\r\n[1,2]";
        let end = find_header_end(raw).unwrap();
        assert_eq!(&raw[end..], b"[1,2]");
        assert_eq!(content_length(&raw[..end]), 363);
        assert_eq!(content_length(b"HTTP/1.1 200 OK\r\n\r\n"), 0);
        assert!(find_header_end(b"partial").is_none());
    }
}
