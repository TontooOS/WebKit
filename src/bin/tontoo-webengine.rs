//! `tontoo-webengine` -- out-of-process web engine helper.
//!
//! Speaks the line protocol from `webkit::transport`: JSON commands
//! (`C {...}`) on stdin, JSON events (`E {...}`) on stdout, BGRA pixels in
//! `<slot-dir>/frame.bgra` announced by `FrameReady`.
//!
//! Renderer selection (`--renderer=mock|gecko|auto`, default `auto`):
//!
//! * `gecko` (feature `gecko`, Firefox binary present): a real headful
//!   Firefox driven over WebDriver BiDi -- navigation, history, input,
//!   JavaScript results, cookies, downloads, dialogs and WebExtensions.
//!   Firefox owns its own Wayland window, so no frames are produced; the
//!   protocol reports `EngineWindow { pid }` instead.
//! * `mock`: animated software placeholder with a mock cookie jar. Used
//!   when no Firefox binary exists and for headless self tests.
//!
//! Usage: `tontoo-webengine --slot-dir <dir> [--ping]
//! [--renderer=...] [--headless] [--private]`.

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{Receiver, sync_channel};

use foundation::serialization::JsonValue;
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
        let mut line = event.to_json_string();
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
    /// Drain asynchronous engine events. Mock is a no-op.
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
                    result: JsonValue::Null,
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
            EngineCommand::InstallExtension { id, path } => {
                emit.emit(&EngineEvent::ExtensionFailed {
                    id,
                    error: format!("mock renderer cannot install {path}"),
                });
            }
            EngineCommand::ListExtensions { id } => {
                emit.emit(&EngineEvent::Extensions {
                    id,
                    extensions: Vec::new(),
                });
            }
            EngineCommand::NewTab { kind, .. } => {
                let context = format!("mock-{kind}-{}", self.tick);
                self.tick += 1;
                emit.emit(&EngineEvent::ContextReady { context });
            }
            EngineCommand::CloseTab { context, .. } => {
                emit.emit(&EngineEvent::ContextClosed { context });
            }
            EngineCommand::ActivateTab { .. } => {}
        }
    }

    fn poll(&mut self, _emit: &mut Emitter) {}

    fn ping(&mut self, emit: &mut Emitter) {
        self.render(emit);
        emit.emit(&EngineEvent::ReadyToShow);
    }
}

// ---------------------------------------------------------------------------
// Gecko renderer (real pages in a headful Firefox)
// ---------------------------------------------------------------------------

#[cfg(feature = "gecko")]
struct GeckoRenderer {
    page: webkit::GeckoPage,
    state: webkit::gecko::GeckoState,
    events: Receiver<webkit::BidiEvent>,
}

#[cfg(feature = "gecko")]
impl GeckoRenderer {
    fn launch(
        download_dir: std::path::PathBuf,
        headless: bool,
        private: bool,
        emit: &mut Emitter,
    ) -> Result<Self, String> {
        let options = webkit::GeckoOptions {
            kiosk: false,
            headless,
            private,
            download_dir,
            window_size: Some((1200, 800)),
            start_url: Some("about:blank".to_string()),
            // The background updater owns provisioning here, so the mock
            // renderer can take over immediately.
            provision: false,
            extensions: Vec::new(),
        };
        let (page, events) = webkit::GeckoPage::launch(options.clone())?;
        let state = webkit::gecko::GeckoState::new(&page, options.download_dir, false);
        // The initial window exists before the event subscription, so
        // report it here.
        emit.emit(&EngineEvent::EngineWindow {
            pid: page.pid(),
            kiosk: false,
        });
        emit.emit(&EngineEvent::ReadyToShow);
        Ok(Self {
            page,
            state,
            events,
        })
    }
}

#[cfg(feature = "gecko")]
impl Renderer for GeckoRenderer {
    fn handle(&mut self, command: EngineCommand, emit: &mut Emitter) {
        let mut out = Vec::new();
        webkit::gecko::run_command(&self.page, command, &mut self.state, &mut out);
        for event in &out {
            emit.emit(event);
        }
    }

    fn poll(&mut self, emit: &mut Emitter) {
        while let Ok(event) = self.events.try_recv() {
            let mut out = Vec::new();
            webkit::gecko::handle_bidi_event(&self.page, &event, &mut self.state, &mut out);
            for event in &out {
                emit.emit(event);
            }
        }
    }

    fn ping(&mut self, emit: &mut Emitter) {
        // A real window is already on screen; report readiness so the
        // helper self test can exit.
        emit.emit(&EngineEvent::EngineWindow {
            pid: self.page.pid(),
            kiosk: self.state.kiosk,
        });
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

fn has_flag(name: &str) -> bool {
    std::env::args().any(|arg| arg == name)
}

fn build_renderer(
    choice: &str,
    slot_dir: &std::path::Path,
    headless: bool,
    private: bool,
    emit: &mut Emitter,
) -> Box<dyn Renderer> {
    #[cfg(feature = "gecko")]
    if choice == "gecko" || choice == "auto" {
        let download_dir = slot_dir.join("downloads");
        let _ = std::fs::create_dir_all(&download_dir);
        match GeckoRenderer::launch(download_dir, headless, private, emit) {
            Ok(renderer) => {
                eprintln!("tontoo-webengine: gecko renderer");
                return Box::new(renderer);
            }
            Err(e) => {
                if choice == "gecko" {
                    emit.emit(&EngineEvent::LoadFailed {
                        url: None,
                        error: format!("firefox unavailable: {e}"),
                    });
                    std::process::exit(1);
                }
                eprintln!("tontoo-webengine: firefox unavailable ({e}); mock renderer");
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
    let headless = has_flag("--headless");
    let private = has_flag("--private");
    // --renderer flag wins, then TONTOO_WEBENGINE_RENDERER, then auto.
    let choice = parse_flag("--renderer=").or_else(|| {
        std::env::var("TONTOO_WEBENGINE_RENDERER")
            .ok()
            .filter(|v| !v.trim().is_empty())
    }).unwrap_or_else(|| "auto".into());
    let Some(slot_dir) = slot_dir else {
        eprintln!("usage: tontoo-webengine --slot-dir <dir> [--ping] [--headless] [--private] [--renderer=mock|gecko|auto]");
        std::process::exit(2);
    };
    let _ = std::fs::create_dir_all(&slot_dir);

    let mut emit = Emitter {
        out: std::io::stdout(),
        slot_dir: slot_dir.clone(),
        seq: 0,
    };
    #[cfg(feature = "gecko")]
    if choice == "auto" || choice == "gecko" {
        // Refresh the managed build at most once a day, in the
        // background; the current build keeps serving meanwhile.
        webkit::gecko_provision::maybe_background_update();
    }
    let mut renderer = build_renderer(&choice, &slot_dir, headless, private, &mut emit);

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
            match EngineCommand::from_json_str(payload) {
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
