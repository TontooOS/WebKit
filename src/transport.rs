//! Out-of-process engine transport.
//!
//! [`ProcessEngine`] spawns the `tontoo-webengine` helper and talks to it
//! over anonymous pipes: JSON commands (`C {...}` lines) on stdin, JSON
//! events (`E {...}` lines) on stdout. Frame pixels travel through a
//! single-slot file in a private temp dir (`frame.bgra` plus the size from
//! the `FrameReady` event), so no socket or fd passing is needed and the
//! same code works on Linux and Windows.
//!
//! The helper binary is located via `TONTOO_WEBENGINE_BIN`, by walking up
//! from the current executable (covers cargo `examples/`, `deps/` and plain
//! `debug/`/`release/` layouts), or on `PATH`. When it cannot be spawned,
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
///
/// Lookup order: `TONTOO_WEBENGINE_BIN`, next to the current executable
/// (walking up through `examples/` and `deps/` layouts that cargo
/// produces), then `PATH`. Note that `cargo run --example` does not build
/// the helper -- run `cargo build` (or `cargo build --bin
/// tontoo-webengine`) once first, otherwise this fails and callers fall
/// back to the test engine.
pub fn find_helper() -> Result<PathBuf, WebKitError> {
    if let Ok(path) = std::env::var("TONTOO_WEBENGINE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(PathBuf::from);
        // Walk up: examples/vello_browser -> examples/ -> debug/ (bins)
        // and deps/vello_browser-hash -> deps/ -> debug/ (bins).
        for _ in 0..3 {
            let Some(current) = dir.clone() else {
                break;
            };
            if let Some(found) = helper_in_dir(&current) {
                return Ok(found);
            }
            dir = current.parent().map(PathBuf::from);
        }
    }
    for name in ["tontoo-webengine", "tontoo-webengine.exe"] {
        if let Some(path) = find_on_path(name) {
            return Ok(path);
        }
    }
    Err(WebKitError::Engine(
        "tontoo-webengine helper not found (run `cargo build --bin tontoo-webengine` or set TONTOO_WEBENGINE_BIN)".into(),
    ))
}

/// Look for the helper directly inside `dir` or inside its `deps/`
/// subdir (cargo hashes test/example binaries there, plain bins land
/// next to it).
fn helper_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    for name in ["tontoo-webengine", "tontoo-webengine.exe"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let deps = dir.join("deps");
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
                    return Some(path);
                }
            }
        }
    }
    None
}

pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
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

/// Build the helper binary once when it is missing. Returns true when a
/// build ran to completion (successfully or not); false when no build
/// was attempted or it had to be killed.
///
/// The build is killed as soon as cargo reports `waiting for file lock`:
/// that means our own parent `cargo run` holds the target dir, and
/// waiting would deadlock. A legit build (fresh checkout, standalone
/// app) runs to completion however long linking takes.
fn try_autobuild() -> bool {
    if std::env::var_os("TONTOO_WEBENGINE_NO_AUTOBUILD").is_some() {
        return false;
    }
    let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") else {
        return false;
    };
    let manifest = PathBuf::from(manifest_dir).join("Cargo.toml");
    if !manifest.is_file() {
        return false;
    }
    eprintln!("tontoo-webengine: helper missing, building it once...");
    let mut child = match Command::new("cargo")
        .arg("build")
        .arg("--bin")
        .arg("tontoo-webengine")
        .arg("--manifest-path")
        .arg(&manifest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("tontoo-webengine: cannot run cargo ({e})");
            return false;
        }
    };
    let stderr = child.stderr.take();
    let contended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = contended.clone();
    let watcher_thread = stderr.map(|stderr| {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                if line.contains("waiting for file lock") {
                    watcher.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
            }
        })
    });
    // Poll for lock contention; a real build just runs to completion.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = watcher_thread.map(|t| t.join());
                if !status.success() {
                    eprintln!("tontoo-webengine: helper build failed");
                }
                return true;
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                return false;
            }
        }
        if contended.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!("tontoo-webengine: build dir locked by parent cargo, skipping");
            let _ = child.kill();
            let _ = child.wait();
            let _ = watcher_thread.map(|t| t.join());
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
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
    ///
    /// When the helper binary is missing (e.g. `cargo run --example`
    /// never builds it), a guarded `cargo build --bin tontoo-webengine`
    /// is attempted once: if cargo reports a contended file lock (the
    /// parent `cargo run` holds it) the build is killed immediately and
    /// the original error is returned instead of deadlocking. Set
    /// `TONTOO_WEBENGINE_NO_AUTOBUILD` to skip the attempt.
    pub fn spawn() -> Result<Self, WebKitError> {
        match find_helper() {
            Ok(bin) => Self::spawn_with_bin(&bin),
            Err(first) => {
                if try_autobuild() {
                    if let Ok(bin) = find_helper() {
                        return Self::spawn_with_bin(&bin);
                    }
                }
                Err(first)
            }
        }
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
                let Ok(event) = EngineEvent::from_json_str(payload) else {
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
        let mut line = command.to_json_string();
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
        let event = EngineEvent::from_json_str(payload).unwrap();
        assert!(matches!(event, EngineEvent::LoadFinished(_)));
        let line = r#"E {"JsResult":{"id":3,"result":null}}"#;
        let payload = line.strip_prefix(EVENT_PREFIX).unwrap();
        let event = EngineEvent::from_json_str(payload).unwrap();
        assert!(matches!(event, EngineEvent::JsResult { id: 3, .. }));
    }

    #[test]
    fn command_lines_roundtrip() {
        let cmd = EngineCommand::Resize { width: 800, height: 600, scale: 1.0 };
        let back = EngineCommand::from_json_str(&cmd.to_json_string()).unwrap();
        assert!(matches!(back, EngineCommand::Resize { width: 800, height: 600, .. }));
        for unit in [
            EngineCommand::Reload,
            EngineCommand::GoBack,
            EngineCommand::Stop,
        ] {
            let back = EngineCommand::from_json_str(&unit.to_json_string()).unwrap();
            assert_eq!(format!("{:?}", back), format!("{:?}", unit));
        }
    }

    #[test]
    fn missing_helper_is_an_error() {
        std::env::remove_var("TONTOO_WEBENGINE_BIN");
        let missing = PathBuf::from("/nonexistent/tontoo-webengine-test-binary");
        assert!(ProcessEngine::spawn_with_bin(&missing).is_err());
    }

    #[test]
    fn find_helper_walks_up_to_bins() {
        // Test binaries live in <target>/debug/deps/, the helper (once
        // `cargo build` ran) in <target>/debug/. Without a built helper
        // this passes trivially instead of failing.
        std::env::remove_var("TONTOO_WEBENGINE_BIN");
        match find_helper() {
            Ok(path) => {
                assert!(path.is_file(), "helper is a file: {}", path.display());
                eprintln!("found helper: {}", path.display());
            }
            Err(e) => {
                eprintln!("no built helper present, skipping ({e})");
            }
        }
    }
}
