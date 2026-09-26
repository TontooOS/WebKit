//! `tontoo-webengine` -- out-of-process web engine helper.
//!
//! Speaks the line protocol from `webkit::transport`: JSON commands
//! (`C {...}`) on stdin, JSON events (`E {...}`) on stdout, BGRA pixels in
//! `<slot-dir>/frame.bgra` announced by `FrameReady`.
//!
//! Current renderer: animated software placeholder (checkerboard with a
//! moving band reacting to input and resizes). The WPE WebKit slot keeps
//! the same protocol: swap `render()` for the
//! `wpe_view_backend_exportable_fdo` readback and everything upstream
//! (view, TontooUI embedding, C ABI) keeps working unchanged.
//!
//! Usage: `tontoo-webengine --slot-dir <dir>`, or `--ping` for a headless
//! single-frame self test.

use std::io::{BufRead, BufReader, Write};

use webkit::engine::{EngineCommand, EngineEvent, SharedFrame};
use webkit::transport::{CMD_PREFIX, EVENT_PREFIX, FRAME_FILE};

struct Helper {
    slot_dir: std::path::PathBuf,
    width: u32,
    height: u32,
    scale: f32,
    seq: u64,
    url: Option<String>,
    cookies: Vec<webkit::cookie::Cookie>,
    out: std::io::Stdout,
}

impl Helper {
    fn emit(&mut self, event: &EngineEvent) {
        let mut line = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
        line.insert_str(0, EVENT_PREFIX);
        line.push('\n');
        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.flush();
    }

    fn render(&mut self) {
        self.seq += 1;
        let mut frame = SharedFrame::checkerboard(self.seq, self.width, self.height);
        // Moving band so input, resizes and the demo visibly animate.
        let band = (self.seq * 24) % (self.width.max(1) * 2) as u64;
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
        let _ = self.scale;
        let _ = std::fs::write(self.slot_dir.join(FRAME_FILE), &frame.rgba);
        self.emit(&EngineEvent::FrameReady {
            seq: frame.seq,
            width: frame.width,
            height: frame.height,
        });
    }

    fn loaded(&mut self, url: Option<String>, title: String) {
        self.url = url.clone();
        self.emit(&EngineEvent::LoadStarted(url.clone()));
        if let Some(url) = url.clone() {
            self.emit(&EngineEvent::Url(Some(url)));
        }
        self.emit(&EngineEvent::Title(Some(title)));
        self.emit(&EngineEvent::Progress(0.3));
        self.render();
        self.emit(&EngineEvent::Progress(1.0));
        self.emit(&EngineEvent::LoadFinished(url));
        self.emit(&EngineEvent::ReadyToShow);
    }

    fn handle(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::LoadUrl(url) => {
                let title = format!("Mock - {url}");
                self.loaded(Some(url), title);
            }
            EngineCommand::LoadHtml { .. } => {
                self.loaded(None, "Mock - local page".into());
            }
            EngineCommand::EvaluateJs { id, .. } => {
                // WPE slot: run in the page JS context and serialize.
                self.emit(&EngineEvent::JsResult {
                    id,
                    result: serde_json::Value::Null,
                });
            }
            EngineCommand::Resize { width, height, scale } => {
                self.width = width.max(1);
                self.height = height.max(1);
                self.scale = scale;
                self.render();
            }
            EngineCommand::MouseDown { .. }
            | EngineCommand::MouseUp { .. }
            | EngineCommand::MouseMove { .. }
            | EngineCommand::Scroll { .. }
            | EngineCommand::KeyText(_) => {
                // WPE slot: forward into the page input pipeline.
                self.render();
            }
            EngineCommand::Reload | EngineCommand::ReloadBypassCache => {
                let url = self.url.clone();
                let title = url
                    .as_deref()
                    .map(|u| format!("Mock - {u}"))
                    .unwrap_or_else(|| "Mock - local page".into());
                self.loaded(url, title);
            }
            EngineCommand::GoBack | EngineCommand::GoForward | EngineCommand::Stop => {}
            EngineCommand::DialogAnswer { .. } | EngineCommand::PermissionAnswer { .. } => {}
            EngineCommand::DownloadDestination { id, path } => {
                if path.is_some() {
                    self.emit(&EngineEvent::DownloadProgress {
                        id,
                        progress: 1.0,
                        received_bytes: 0,
                    });
                    self.emit(&EngineEvent::DownloadFinished { id });
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
                self.emit(&EngineEvent::Cookies {
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
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let slot_dir = args
        .windows(2)
        .find(|w| w[0] == "--slot-dir")
        .map(|w| std::path::PathBuf::from(&w[1]));
    let ping = args.iter().any(|a| a == "--ping");
    let Some(slot_dir) = slot_dir else {
        eprintln!("usage: tontoo-webengine --slot-dir <dir> [--ping]");
        std::process::exit(2);
    };
    let _ = std::fs::create_dir_all(&slot_dir);

    let mut helper = Helper {
        slot_dir,
        width: 800,
        height: 600,
        scale: 1.0,
        seq: 0,
        url: None,
        cookies: Vec::new(),
        out: std::io::stdout(),
    };

    if ping {
        helper.render();
        helper.emit(&EngineEvent::ReadyToShow);
        return;
    }

    eprintln!("tontoo-webengine ready");
    let stdin = std::io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        let Some(payload) = line.strip_prefix(CMD_PREFIX) else {
            continue;
        };
        match serde_json::from_str::<EngineCommand>(payload) {
            Ok(command) => helper.handle(command),
            Err(e) => eprintln!("bad command: {e}"),
        }
    }
}
