//! `tontoo-webengine` -- out-of-process web engine helper.
//!
//! Speaks the line protocol from `webkit::transport`: JSON commands
//! (`C {...}`) on stdin, JSON events (`E {...}`) on stdout, BGRA pixels in
//! `<slot-dir>/frame.bgra` announced by `FrameReady`.
//!
//! Renderer selection (`--renderer=mock|chromium|auto`, default `auto`):
//!
//! * `chromium` (feature `chromium`, Chromium binary present): real pages
//!   inside the Chromium sandbox via the DevTools protocol -- navigation,
//!   screenshots, input, JavaScript results, cookies, downloads, dialogs.
//! * `mock`: animated software placeholder with a mock cookie jar. Used
//!   when no Chromium binary exists and for headless self tests.
//!
//! Usage: `tontoo-webengine --slot-dir <dir> [--ping]
//! [--renderer=...]`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{Receiver, sync_channel};

use webkit::cookie::Cookie;
use webkit::engine::{EngineCommand, EngineEvent, SharedFrame};
use webkit::transport::{CMD_PREFIX, EVENT_PREFIX, FRAME_FILE};

/// Protocol shell: event lines plus single-slot frame files.
struct Emitter {
    out: std::io::Stdout,
    slot_dir: std::path::PathBuf,
    seq: u64,
}

impl Emitter {
    fn emit(&mut self, event: &EngineEvent) {
        let mut line = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
        line.insert_str(0, EVENT_PREFIX);
        line.push('\n');
        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.flush();
    }

    fn frame(&mut self, rgba: &[u8], width: u32, height: u32) {
        self.seq += 1;
        let _ = std::fs::write(self.slot_dir.join(FRAME_FILE), rgba);
        self.emit(&EngineEvent::FrameReady {
            seq: self.seq,
            width,
            height,
        });
    }
}

/// Page renderer behind the protocol shell.
trait Renderer {
    /// Handle one UI command.
    fn handle(&mut self, command: EngineCommand, emit: &mut Emitter);
    /// Drain asynchronous engine events (CDP callbacks). Mock is a no-op.
    fn poll(&mut self, emit: &mut Emitter);
    /// Render one frame immediately (`--ping`).
    fn ping(&mut self, emit: &mut Emitter);
}

// ---------------------------------------------------------------------------
// Mock renderer
// ---------------------------------------------------------------------------

struct MockRenderer {
    width: u32,
    height: u32,
    url: Option<String>,
    cookies: Vec<Cookie>,
    tick: u64,
}

impl MockRenderer {
    fn new() -> Self {
        Self {
            width: 800,
            height: 600,
            url: None,
            cookies: Vec::new(),
            tick: 0,
        }
    }

    fn render(&mut self, emit: &mut Emitter) {
        self.tick += 1;
        let mut frame = SharedFrame::checkerboard(0, self.width, self.height);
        // Moving band so input, resizes and the demo visibly animate.
        let band = (self.tick * 24) % (self.width.max(1) * 2) as u64;
        for y in 0..self.height {
            for x in 0..self.width {
                let d = x as i64 - band as i64 + y as i64 / 2;
                if d >= 0 && d < 48 {
                    let i = (y as usize * self.width as usize + x as usize) * 4;
                    frame.rgba[i] = 0xff;
                    frame.rgba[i + 1] = 0x9f;
                    frame.rgba[i + 2] = 0x0a;
                    frame.rgba[i + 3] = 0xff;
                }
            }
        }
        // Address bar strip: darker top rows echo the current URL length.
        let strip = 28.min(self.height) as usize;
        let fill = self.url.as_ref().map(|u| u.len() % 256).unwrap_or(0) as u8;
        for y in 0..strip {
            for x in 0..self.width as usize {
                let i = (y * self.width as usize + x) * 4;
                frame.rgba[i] = 0x10 + fill / 4;
                frame.rgba[i + 1] = 0x14;
                frame.rgba[i + 2] = 0x16;
                frame.rgba[i + 3] = 0xff;
            }
        }
        emit.frame(&frame.rgba, frame.width, frame.height);
    }

    fn loaded(&mut self, url: Option<String>, title: String, emit: &mut Emitter) {
        self.url = url.clone();
        emit.emit(&EngineEvent::LoadStarted(url.clone()));
        if let Some(url) = url.clone() {
            emit.emit(&EngineEvent::Url(Some(url)));
        }
        emit.emit(&EngineEvent::Title(Some(title)));
        emit.emit(&EngineEvent::Progress(0.3));
        self.render(emit);
        emit.emit(&EngineEvent::Progress(1.0));
        emit.emit(&EngineEvent::LoadFinished(url));
        emit.emit(&EngineEvent::History {
            can_back: false,
            can_forward: false,
        });
        emit.emit(&EngineEvent::ReadyToShow);
    }
}

impl Renderer for MockRenderer {
    fn handle(&mut self, command: EngineCommand, emit: &mut Emitter) {
        match command {
            EngineCommand::LoadUrl(url) => {
                let title = format!("Mock - {url}");
                self.loaded(Some(url), title, emit);
            }
            EngineCommand::LoadHtml { .. } => {
                self.loaded(None, "Mock - local page".into(), emit);
            }
            EngineCommand::EvaluateJs { id, .. } => {
                emit.emit(&EngineEvent::JsResult {
                    id,
                    result: serde_json::Value::Null,
                });
            }
            EngineCommand::Resize { width, height, .. } => {
                self.width = width.max(1);
                self.height = height.max(1);
                self.render(emit);
            }
            EngineCommand::MouseDown { .. }
            | EngineCommand::MouseUp { .. }
            | EngineCommand::MouseMove { .. }
            | EngineCommand::Scroll { .. }
            | EngineCommand::KeyText(_)
            | EngineCommand::SpecialKey { .. } => {
                self.render(emit);
            }
            EngineCommand::Reload | EngineCommand::ReloadBypassCache => {
                let url = self.url.clone();
                let title = url
                    .as_deref()
                    .map(|u| format!("Mock - {u}"))
                    .unwrap_or_else(|| "Mock - local page".into());
                self.loaded(url, title, emit);
            }
            EngineCommand::GoBack | EngineCommand::GoForward | EngineCommand::Stop => {}
            EngineCommand::DialogAnswer { .. } | EngineCommand::PermissionAnswer { .. } => {}
            EngineCommand::DownloadDestination { id, path } => {
                if path.is_some() {
                    emit.emit(&EngineEvent::DownloadProgress {
                        id,
                        progress: 1.0,
                        received_bytes: 0,
                    });
                    emit.emit(&EngineEvent::DownloadFinished { id });
                }
            }
            EngineCommand::CancelDownload { .. } => {}
            EngineCommand::ClearData { cookies, cache } => {
                if cookies {
                    self.cookies.clear();
                }
                let _ = cache;
            }
            EngineCommand::ListCookies { id } => {
                emit.emit(&EngineEvent::Cookies {
                    id,
                    cookies: self.cookies.clone(),
                });
            }
            EngineCommand::AddCookie { cookie } => {
                if let Some(existing) = self.cookies.iter_mut().find(|c| {
                    c.domain == cookie.domain && c.path == cookie.path && c.name == cookie.name
                }) {
                    *existing = cookie;
                } else {
                    self.cookies.push(cookie);
                }
            }
            EngineCommand::DeleteCookie { domain, path, name } => {
                self.cookies
                    .retain(|c| !(c.domain == domain && c.path == path && c.name == name));
            }
        }
    }

    fn poll(&mut self, _emit: &mut Emitter) {}

    fn ping(&mut self, emit: &mut Emitter) {
        self.render(emit);
        emit.emit(&EngineEvent::ReadyToShow);
    }
}

// ---------------------------------------------------------------------------
// Chromium renderer (real pages in the Chromium sandbox)
// ---------------------------------------------------------------------------

#[cfg(feature = "chromium")]
struct ChromiumRenderer {
    page: webkit::chromium::CdpPage,
    events: Receiver<webkit::chromium::CdpEvent>,
    download_dir: std::path::PathBuf,
    url: Option<String>,
    last_pos: (f64, f64),
    next_id: u64,
    /// Engine download id -> (CDP guid, chosen destination or None).
    downloads: HashMap<u64, (String, Option<String>)>,
    guids: HashMap<String, u64>,
}

#[cfg(feature = "chromium")]
impl ChromiumRenderer {
    fn launch(download_dir: std::path::PathBuf) -> Result<Self, String> {
        let (page, events) = webkit::chromium::CdpPage::launch(download_dir.clone())?;
        Ok(Self {
            page,
            events,
            download_dir,
            url: None,
            last_pos: (0.0, 0.0),
            next_id: 1,
            downloads: HashMap::new(),
            guids: HashMap::new(),
        })
    }

    fn capture(&self, emit: &mut Emitter) {
        match self.page.capture() {
            Ok((bgra, w, h)) => {
                emit.frame(&bgra, w, h);
                if let Ok((back, fwd)) = self.page.history_state() {
                    emit.emit(&EngineEvent::History {
                        can_back: back,
                        can_forward: fwd,
                    });
                }
            }
            Err(e) => eprintln!("chromium: capture failed ({e})"),
        }
    }

    fn finish_load(&self, url: Option<String>, emit: &mut Emitter) {
        let title = self.page.title();
        emit.emit(&EngineEvent::Title(title));
        emit.emit(&EngineEvent::Progress(1.0));
        emit.emit(&EngineEvent::LoadFinished(url));
        emit.emit(&EngineEvent::ReadyToShow);
        self.capture(emit);
    }

    fn finish_download(&mut self, id: u64, emit: &mut Emitter) {
        let Some((guid, dest)) = self.downloads.remove(&id) else {
            return;
        };
        self.guids.remove(&guid);
        let Some(dest) = dest else {
            // No destination chosen (delegate cancelled after the fact).
            emit.emit(&EngineEvent::DownloadFailed {
                id,
                error: "cancelled".into(),
            });
            return;
        };
        // Chrome may suffix collisions (`name (1)`); take the newest match.
        // Downloaded files land directly in the download dir.
        std::thread::sleep(std::time::Duration::from_millis(200));
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
                    Ok(()) => emit.emit(&EngineEvent::DownloadFinished { id }),
                    Err(e) => emit.emit(&EngineEvent::DownloadFailed {
                        id,
                        error: format!("save failed: {e}"),
                    }),
                }
            }
            None => emit.emit(&EngineEvent::DownloadFailed {
                id,
                error: "download file missing".into(),
            }),
        }
    }
}

#[cfg(feature = "chromium")]
impl Renderer for ChromiumRenderer {
    fn handle(&mut self, command: EngineCommand, emit: &mut Emitter) {
        match command {
            EngineCommand::LoadUrl(url) => {
                self.url = Some(url.clone());
                emit.emit(&EngineEvent::LoadStarted(Some(url.clone())));
                emit.emit(&EngineEvent::Url(Some(url.clone())));
                emit.emit(&EngineEvent::Progress(0.2));
                if let Err(e) = self.page.navigate(&url) {
                    emit.emit(&EngineEvent::LoadFailed {
                        url: Some(url),
                        error: e,
                    });
                }
            }
            EngineCommand::LoadHtml { html, .. } => {
                self.url = None;
                emit.emit(&EngineEvent::LoadStarted(None));
                if let Err(e) = self.page.set_document_content(&html) {
                    emit.emit(&EngineEvent::LoadFailed { url: None, error: e });
                }
            }
            EngineCommand::EvaluateJs { id, script } => {
                match self.page.evaluate(&script) {
                    Ok(result) => emit.emit(&EngineEvent::JsResult { id, result }),
                    Err(e) => {
                        eprintln!("chromium: evaluate failed ({e})");
                        emit.emit(&EngineEvent::JsResult {
                            id,
                            result: serde_json::Value::Null,
                        });
                    }
                }
            }
            EngineCommand::Resize { width, height, scale } => {
                if self.page.set_viewport(width, height, scale).is_ok() {
                    self.capture(emit);
                }
            }
            EngineCommand::MouseDown { x, y } => {
                self.last_pos = (x, y);
                let _ = self.page.mouse_move(x, y);
                let _ = self.page.mouse_button(x, y, true);
                self.capture(emit);
            }
            EngineCommand::MouseUp { x, y } => {
                self.last_pos = (x, y);
                let _ = self.page.mouse_button(x, y, false);
                self.capture(emit);
            }
            EngineCommand::MouseMove { x, y } => {
                self.last_pos = (x, y);
                let _ = self.page.mouse_move(x, y);
            }
            EngineCommand::Scroll { dx, dy } => {
                let (x, y) = self.last_pos;
                let _ = self.page.wheel(x, y, dx, dy);
                self.capture(emit);
            }
            EngineCommand::KeyText(text) => {
                let _ = self.page.insert_text(&text);
                self.capture(emit);
            }
            EngineCommand::SpecialKey { key } => {
                let _ = self.page.special_key(&key);
                self.capture(emit);
            }
            EngineCommand::Reload => {
                let _ = self.page.reload(false);
            }
            EngineCommand::ReloadBypassCache => {
                let _ = self.page.reload(true);
            }
            EngineCommand::GoBack => {
                let _ = self.page.go_history(-1);
            }
            EngineCommand::GoForward => {
                let _ = self.page.go_history(1);
            }
            EngineCommand::Stop => {
                let _ = self.page.stop_loading();
            }
            EngineCommand::DialogAnswer { confirmed, text, .. } => {
                let _ = self.page.dialog_answer(confirmed, text.as_deref());
            }
            EngineCommand::PermissionAnswer { .. } => {
                // Chromium auto-denies prompts unless pre-granted; this
                // driver never pre-grants, so permissions fail closed.
            }
            EngineCommand::DownloadDestination { id, path } => {
                if let Some((guid, slot)) = self.downloads.get_mut(&id) {
                    if path.is_none() {
                        let guid = guid.clone();
                        let _ = self.page.cancel_download(&guid);
                        self.downloads.remove(&id);
                        self.guids.remove(&guid);
                        emit.emit(&EngineEvent::DownloadFailed {
                            id,
                            error: "cancelled".into(),
                        });
                    } else {
                        *slot = path;
                    }
                }
            }
            EngineCommand::CancelDownload { id } => {
                if let Some((guid, _)) = self.downloads.remove(&id) {
                    let _ = self.page.cancel_download(&guid);
                    self.guids.remove(&guid);
                }
            }
            EngineCommand::ClearData { cookies, cache } => {
                let origin = self.url.clone();
                let _ = self.page.clear_data(cookies, cache, origin.as_deref());
            }
            EngineCommand::ListCookies { id } => {
                match self.page.list_cookies() {
                    Ok(cookies) => emit.emit(&EngineEvent::Cookies { id, cookies }),
                    Err(e) => eprintln!("chromium: cookies failed ({e})"),
                }
            }
            EngineCommand::AddCookie { cookie } => {
                let _ = self.page.add_cookie(&cookie);
            }
            EngineCommand::DeleteCookie { domain, path, name } => {
                let _ = self.page.delete_cookie(&domain, &path, &name);
            }
        }
    }

    fn poll(&mut self, emit: &mut Emitter) {
        use webkit::chromium::CdpEvent as C;
        while let Ok(event) = self.events.try_recv() {
            match event {
                C::Started { url } => {
                    emit.emit(&EngineEvent::LoadStarted(Some(url)));
                    emit.emit(&EngineEvent::Progress(0.2));
                }
                C::Navigated { url, .. } => {
                    self.url = Some(url.clone());
                    emit.emit(&EngineEvent::Url(Some(url)));
                }
                C::Loaded => {
                    let url = self.url.clone();
                    self.finish_load(url, emit);
                }
                C::Dialog { dialog_type, message, default_prompt } => {
                    let id = self.next_id;
                    self.next_id += 1;
                    let kind = match dialog_type.as_str() {
                        "confirm" => webkit::delegate::ScriptDialogKind::Confirm,
                        "prompt" => webkit::delegate::ScriptDialogKind::Prompt,
                        "beforeunload" => webkit::delegate::ScriptDialogKind::BeforeUnloadConfirm,
                        _ => webkit::delegate::ScriptDialogKind::Alert,
                    };
                    // CDP answers the live dialog directly; the engine id
                    // only correlates the delegate round-trip in the UI.
                    emit.emit(&EngineEvent::ScriptDialog {
                        id,
                        kind,
                        message,
                        prompt_default: if default_prompt.is_empty() {
                            None
                        } else {
                            Some(default_prompt)
                        },
                    });
                }
                C::DownloadBegin { guid, url, filename } => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.downloads.insert(id, (guid.clone(), None));
                    self.guids.insert(guid, id);
                    emit.emit(&EngineEvent::DownloadStarted {
                        id,
                        uri: Some(url),
                        suggested_filename: filename,
                    });
                }
                C::DownloadProgress { guid, received, state } => {
                    let Some(id) = self.guids.get(&guid).copied() else {
                        continue;
                    };
                    match state.as_str() {
                        "completed" => self.finish_download(id, emit),
                        "canceled" => {
                            self.downloads.remove(&id);
                            self.guids.remove(&guid);
                            emit.emit(&EngineEvent::DownloadFailed {
                                id,
                                error: "canceled".into(),
                            });
                        }
                        _ => emit.emit(&EngineEvent::DownloadProgress {
                            id,
                            progress: 0.0,
                            received_bytes: received,
                        }),
                    }
                }
            }
        }
    }

    fn ping(&mut self, emit: &mut Emitter) {
        self.capture(emit);
        emit.emit(&EngineEvent::ReadyToShow);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn parse_flag(prefix: &str) -> Option<String> {
    std::env::args().find_map(|arg| {
        arg.strip_prefix(prefix)
            .map(|value| value.trim_matches(|c| c == '"' || c == '\'').to_string())
    })
}

fn build_renderer(
    choice: &str,
    slot_dir: &std::path::Path,
    emit: &mut Emitter,
) -> Box<dyn Renderer> {
    #[cfg(feature = "chromium")]
    if choice == "chromium" || choice == "auto" {
        let download_dir = slot_dir.join("downloads");
        let _ = std::fs::create_dir_all(&download_dir);
        match ChromiumRenderer::launch(download_dir) {
            Ok(renderer) => {
                eprintln!("tontoo-webengine: chromium renderer");
                return Box::new(renderer);
            }
            Err(e) => {
                if choice == "chromium" {
                    emit.emit(&EngineEvent::LoadFailed {
                        url: None,
                        error: format!("chromium unavailable: {e}"),
                    });
                    std::process::exit(1);
                }
                eprintln!("tontoo-webengine: chromium unavailable ({e}); mock renderer");
            }
        }
    }
    let _ = choice;
    let _ = slot_dir;
    eprintln!("tontoo-webengine: mock renderer");
    Box::new(MockRenderer::new())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let slot_dir = args
        .windows(2)
        .find(|w| w[0] == "--slot-dir")
        .map(|w| std::path::PathBuf::from(&w[1]));
    let ping = args.iter().any(|a| a == "--ping");
    // --renderer flag wins, then TONTOO_WEBENGINE_RENDERER, then auto.
    let choice = parse_flag("--renderer=").or_else(|| {
        std::env::var("TONTOO_WEBENGINE_RENDERER")
            .ok()
            .filter(|v| !v.trim().is_empty())
    }).unwrap_or_else(|| "auto".into());
    let Some(slot_dir) = slot_dir else {
        eprintln!("usage: tontoo-webengine --slot-dir <dir> [--ping] [--renderer=mock|chromium|auto]");
        std::process::exit(2);
    };
    let _ = std::fs::create_dir_all(&slot_dir);

    let mut emit = Emitter {
        out: std::io::stdout(),
        slot_dir: slot_dir.clone(),
        seq: 0,
    };
    let mut renderer = build_renderer(&choice, &slot_dir, &mut emit);

    if ping {
        renderer.ping(&mut emit);
        return;
    }

    eprintln!("tontoo-webengine ready");
    let (cmd_tx, cmd_rx) = sync_channel::<EngineCommand>(256);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in BufReader::new(stdin.lock()).lines() {
            let Ok(line) = line else { break };
            let Some(payload) = line.strip_prefix(CMD_PREFIX) else {
                continue;
            };
            match serde_json::from_str::<EngineCommand>(payload) {
                Ok(command) => {
                    if cmd_tx.try_send(command).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("bad command: {e}"),
            }
        }
    });

    loop {
        use std::sync::mpsc::TryRecvError;
        let mut idle = true;
        loop {
            match cmd_rx.try_recv() {
                Ok(command) => {
                    idle = false;
                    renderer.handle(command, &mut emit);
                }
                Err(TryRecvError::Empty) => break,
                // Stdin closed and queue drained: flush async events, exit.
                Err(TryRecvError::Disconnected) => {
                    renderer.poll(&mut emit);
                    return;
                }
            }
        }
        renderer.poll(&mut emit);
        if idle {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
