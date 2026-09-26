//! Out-of-process engine transport.
//!
//! [`ProcessEngine`] spawns the `tontoo-webengine` helper and talks to it
//! over anonymous pipes: JSON commands (`C {...}` lines) on stdin, JSON
//! events (`E {...}` lines) on stdout. Frame pixels travel through a
//! single-slot file in a private temp dir (`frame.bgra` plus the size from
//! the `FrameReady` event), so no socket or fd passing is needed and the
//! same code works on Linux and Windows.
//!
//! The helper binary is located via `TONTOO_WEBENGINE_BIN`, next to the
//! current executable, or on `PATH`. When it cannot be spawned,
//! [`ProcessEngine::spawn`] fails and callers (e.g.
//! [`crate::view::WebView::with_spawned_engine`]) fall back to the test
//! engine.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::engine::{EngineCommand, EngineEvent, SharedFrame, WebEngine};
use crate::error::WebKitError;

/// Line prefix for commands (UI -> helper) and events (helper -> UI).
pub const CMD_PREFIX: &str = "C ";
pub const EVENT_PREFIX: &str = "E ";

/// Name of the pixel file inside the slot dir (raw BGRA, no header).
pub const FRAME_FILE: &str = "frame.bgra";

/// Locate the helper binary.
pub fn find_helper() -> Result<PathBuf, WebKitError> {
    if let Ok(path) = std::env::var("TONTOO_WEBENGINE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["tontoo-webengine", "tontoo-webengine.exe"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
            // Cargo places helper bins next to examples under deps/.
            let deps = dir.join("deps");
            if deps.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&deps) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if name == "tontoo-webengine"
                            || name == "tontoo-webengine.exe"
                            || name.starts_with("tontoo-webengine-")
                        {
                            let path = entry.path();
                            if path.is_file() {
                                return Ok(path);
                            }
                        }
                    }
                }
            }
        }
    }
    for name in ["tontoo-webengine", "tontoo-webengine.exe"] {
        if let Some(path) = find_on_path(name) {
            return Ok(path);
        }
    }
    Err(WebKitError::Engine(
        "tontoo-webengine helper not found (set TONTOO_WEBENGINE_BIN)".into(),
    ))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in std::env::split_paths(&path) {
        let _ = sep;
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// A [`WebEngine`] implementation backed by the helper child process.
pub struct ProcessEngine {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    events: Arc<Mutex<Vec<EngineEvent>>>,
    frame: Arc<Mutex<Option<Arc<SharedFrame>>>>,
    slot_dir: PathBuf,
}

impl ProcessEngine {
    /// Spawn the helper located by [`find_helper`].
    pub fn spawn() -> Result<Self, WebKitError> {
        let bin = find_helper()?;
        Self::spawn_with_bin(&bin)
    }

    /// Spawn a specific helper binary (used by tests and the demo).
    pub fn spawn_with_bin(bin: &std::path::Path) -> Result<Self, WebKitError> {
        let slot_dir = std::env::temp_dir().join(format!(
            "tontoo-web-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&slot_dir)
            .map_err(|e| WebKitError::Engine(format!("slot dir: {e}")))?;
        let mut child = Command::new(bin)
            .arg("--slot-dir")
            .arg(&slot_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| WebKitError::Engine(format!("spawn helper: {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| {
            WebKitError::Engine("helper stdin unavailable".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            WebKitError::Engine("helper stdout unavailable".into())
        })?;
        let engine = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            events: Arc::new(Mutex::new(Vec::new())),
            frame: Arc::new(Mutex::new(None)),
            slot_dir: slot_dir.clone(),
        };
        engine.start_reader(stdout, slot_dir);
        Ok(engine)
    }

    fn start_reader(
        &self,
        stdout: std::process::ChildStdout,
        slot_dir: PathBuf,
    ) {
        let events = Arc::clone(&self.events);
        let frame = Arc::clone(&self.frame);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Some(payload) = line.strip_prefix(EVENT_PREFIX) else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<EngineEvent>(payload) else {
                    continue;
                };
                if let EngineEvent::FrameReady { seq, width, height } = &event {
                    let path = slot_dir.join(FRAME_FILE);
                    if let Ok(rgba) = std::fs::read(&path) {
                        if rgba.len() == *width as usize * *height as usize * 4 {
                            *frame.lock().expect("frame lock") = Some(Arc::new(SharedFrame {
                                seq: *seq,
                                width: *width,
                                height: *height,
                                rgba,
                            }));
                        }
                    }
                } else {
                    events.lock().expect("events lock").push(event);
                }
            }
        });
    }
}

impl Drop for ProcessEngine {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.slot_dir);
    }
}

impl WebEngine for ProcessEngine {
    fn send(&self, command: EngineCommand) {
        let mut line = serde_json::to_string(&command).unwrap_or_else(|_| "{}".into());
        line.insert_str(0, CMD_PREFIX);
        line.push('\n');
        if let Ok(mut stdin) = self.stdin.lock() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.flush();
        }
    }

    fn latest_frame(&self) -> Option<Arc<SharedFrame>> {
        self.frame.lock().expect("frame lock").clone()
    }

    fn drain_events(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut *self.events.lock().expect("events lock"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_lines_parse() {
        let line = r#"E {"LoadFinished":"https://example.com"}"#;
        let payload = line.strip_prefix(EVENT_PREFIX).unwrap();
        let event: EngineEvent = serde_json::from_str(payload).unwrap();
        assert!(matches!(event, EngineEvent::LoadFinished(_)));
        let line = r#"E {"JsResult":{"id":3,"result":null}}"#;
        let payload = line.strip_prefix(EVENT_PREFIX).unwrap();
        let event: EngineEvent = serde_json::from_str(payload).unwrap();
        assert!(matches!(event, EngineEvent::JsResult { id: 3, .. }));
    }

    #[test]
    fn missing_helper_is_an_error() {
        std::env::remove_var("TONTOO_WEBENGINE_BIN");
        let missing = PathBuf::from("/nonexistent/tontoo-webengine-test-binary");
        assert!(ProcessEngine::spawn_with_bin(&missing).is_err());
    }
}
