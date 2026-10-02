//! Self-provisioning Firefox ESR builds (feature `gecko`).
//!
//! TontooOS runs a Firefox even when no distro package provides one. This
//! module mirrors the Chromium auto-update that shipped before: when no
//! usable Firefox binary exists, download the current ESR build from
//! Mozilla, unpack it into a managed dir and smoke-test it (`firefox
//! --version` must exit 0). Only runnable builds are ever installed, so an
//! old glibc host (WSL containers) simply lands on nothing and falls back
//! to the system browser.
//!
//! Layout (per user, shared by all helper instances):
//!
//! ```text
//! <managed>/VERSION              installed version, e.g. 140.17.0esr
//! <managed>/<version>/firefox/   extracted tarball (Linux)
//! <managed>/LAST_UPDATE_CHECK    unix millis of the last update poll
//! ```
//!
//! Env: `TONTOO_FIREFOX_DIR` overrides the managed dir,
//! `TONTOO_FIREFOX_AUTOUPDATE=0` disables all network provisioning,
//! `TONTOO_FIREFOX_CHANNEL=release` switches from `esr` to `release`.

#![cfg(feature = "gecko")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Mozilla product-details document with every release channel.
pub const PRODUCT_DETAILS: &str =
    "https://product-details.mozilla.org/1.0/firefox_versions.json";
/// Release directory template on the Mozilla FTP mirror.
pub const RELEASE_BASE: &str = "https://ftp.mozilla.org/pub/firefox/releases";

const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// How many ESR releases to try before giving up.
const MAX_LADDER_STEPS: usize = 8;

/// Release channel used for new installs: `esr` (default) or `release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Esr,
    Release,
}

impl Channel {
    /// Channel from `TONTOO_FIREFOX_CHANNEL`, defaulting to ESR.
    pub fn from_env() -> Self {
        match std::env::var("TONTOO_FIREFOX_CHANNEL").as_deref() {
            Ok("release") | Ok("Release") | Ok("RELEASE") => Channel::Release,
            _ => Channel::Esr,
        }
    }

    /// JSON field holding this channel's version.
    pub fn version_field(self) -> &'static str {
        match self {
            Channel::Esr => "FIREFOX_ESR",
            Channel::Release => "LATEST_FIREFOX_VERSION",
        }
    }
}

/// Network provisioning allowed unless explicitly disabled.
pub fn autoupdate_enabled() -> bool {
    std::env::var("TONTOO_FIREFOX_AUTOUPDATE").as_deref() != Ok("0")
}

/// Managed install dir: override, Windows LocalAppData, or XDG data home.
pub fn managed_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TONTOO_FIREFOX_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local)
                .join("TontooWebEngine")
                .join("gecko");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    PathBuf::from(base).join("tontoo-webengine").join("gecko")
}

/// Mozilla platform directory for this host.
///
/// Only Linux hosts are provisioned: Firefox ships a tarball there, while
/// Windows and macOS distribute installers and `.dmg` images that do not
/// belong inside a managed dir. On those platforms the system Firefox is
/// the only supported source.
pub fn platform_slug() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        _ => None,
    }
}

/// Binary path inside an extracted tarball for this host.
pub fn binary_relpath() -> Option<&'static str> {
    platform_slug().map(|_| "firefox/firefox")
}

/// Installed version from the VERSION file, if the binary still exists.
pub fn installed() -> Option<(String, PathBuf)> {
    let dir = managed_dir();
    let version = std::fs::read_to_string(dir.join("VERSION")).ok()?;
    let version = version.trim().to_string();
    if version.is_empty() {
        return None;
    }
    let bin = dir.join(&version).join(binary_relpath()?);
    if bin.is_file() {
        Some((version, bin))
    } else {
        None
    }
}

/// Quick smoke test: the loader and version reporting must work. This is
/// what rejects builds needing a newer glibc than the host provides.
pub fn smoke_test(bin: &Path) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Short-timeout agent for small JSON documents.
fn text_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

/// Long-timeout agent for the browser tarball (~80 MB compressed).
fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30 * 60)))
        .build()
        .into()
}

/// GET a URL as text.
fn http_get_text(url: &str) -> Result<String, String> {
    let mut response = text_agent().get(url).call().map_err(|e| e.to_string())?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| e.to_string())
        .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
}

/// Read a version field out of product-details JSON.
pub(crate) fn parse_version(
    json: &foundation::serialization::JsonValue,
    field: &str,
) -> Option<String> {
    json.get(field)?.as_str().map(str::to_string)
}

/// Fetch the newest version of `channel`.
pub fn fetch_version(channel: Channel) -> Result<String, String> {
    let text = http_get_text(PRODUCT_DETAILS)?;
    let json = foundation::serialization::JsonValue::parse(&text)
        .map_err(|e| format!("product details json: {e}"))?;
    parse_version(&json, channel.version_field())
        .ok_or_else(|| format!("product details without {}", channel.version_field()))
}

/// Older versions of the same channel, newest first.
///
/// `FIREFOX_ESR` only ever reports the current branch, so the ladder is
/// built by stepping the minor number down until a download URL answers.
/// Used only when the newest build fails its smoke test.
fn version_ladder(newest: &str) -> Vec<String> {
    let (major, rest) = match newest.split_once('.') {
        Some(parts) => parts,
        None => return vec![newest.to_string()],
    };
    let Some((minor, tail)) = rest.split_once('.') else {
        return vec![newest.to_string()];
    };
    let Ok(minor) = minor.parse::<u32>() else {
        return vec![newest.to_string()];
    };
    let esr = tail.trim_start_matches("0").starts_with("esr");
    let mut out = Vec::new();
    // ESR minors are not contiguous (128.0 -> 128.1 -> ... -> 128.14),
    // so step down generously but bound the walk.
    let floor = minor.saturating_sub(MAX_LADDER_STEPS as u32 * 4);
    let mut candidate = minor;
    while candidate > floor && out.len() < MAX_LADDER_STEPS {
        out.push(if esr {
            format!("{major}.{candidate}.0esr")
        } else {
            format!("{major}.{candidate}.0")
        });
        candidate -= 1;
    }
    out
}

/// Download URLs for one version, most specific first.
fn download_urls(version: &str, platform: &str) -> Vec<String> {
    let name = format!("firefox-{version}");
    vec![
        format!("{RELEASE_BASE}/{version}/{platform}/en-US/{name}.tar.xz"),
        format!("{RELEASE_BASE}/{version}/{platform}/en-US/{name}.tar.bz2"),
    ]
}

/// Download a Firefox tarball and unpack it into the managed dir.
/// Returns the binary path on success (smoke-tested).
fn download_version(version: &str) -> Result<PathBuf, String> {
    let dir = managed_dir();
    let Some(platform) = platform_slug() else {
        return Err("Firefox auto-provisioning is Linux-only".into());
    };
    let mut last = String::new();
    let mut bytes = None;
    for url in download_urls(version, platform) {
        match download_agent().get(&url).call() {
            Ok(mut response) => {
                eprintln!("tontoo-webengine: downloading Firefox {version} ({platform})...");
                let data = response
                    .body_mut()
                    .with_config()
                    .limit(2 << 30)
                    .read_to_vec()
                    .map_err(|e| format!("download: {e}"))?;
                eprintln!(
                    "tontoo-webengine: downloaded {} MB, unpacking...",
                    data.len() / 1_048_576
                );
                bytes = Some(data);
                break;
            }
            Err(e) => last = format!("{url}: {e}"),
        }
    }
    let Some(bytes) = bytes else {
        return Err(if last.is_empty() {
            "no download URL".into()
        } else {
            last
        });
    };

    let pending = dir.join(format!("pending-{version}"));
    let _ = std::fs::remove_dir_all(&pending);
    std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
    unpack_xz(&bytes, &pending).or_else(|e| {
        let _ = std::fs::remove_dir_all(&pending);
        Err(e)
    })?;
    fix_exec_bits(&pending);

    let Some(rel) = binary_relpath() else {
        let _ = std::fs::remove_dir_all(&pending);
        return Err("Firefox auto-provisioning is Linux-only".into());
    };
    let bin = pending.join(rel);
    if !bin.is_file() {
        let _ = std::fs::remove_dir_all(&pending);
        return Err("tarball without the firefox binary".into());
    }
    if !smoke_test(&bin) {
        let _ = std::fs::remove_dir_all(&pending);
        return Err(format!("Firefox {version} does not run here"));
    }
    let target = dir.join(version);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&pending, &target).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("VERSION"), version).map_err(|e| e.to_string())?;
    prune_old_versions(&dir, version);
    Ok(target.join(rel))
}

/// Unpack an xz-compressed tarball into `dest`.
fn unpack_xz(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut source = data;
    let mut decoded = Vec::new();
    lzma_rs::xz_decompress(&mut source, &mut decoded).map_err(|e| format!("xz: {e}"))?;
    let mut archive = tar::Archive::new(std::io::Cursor::new(decoded));
    archive.set_overwrite(true);
    archive.unpack(dest).map_err(|e| format!("tar: {e}"))?;
    Ok(())
}

#[cfg(unix)]
fn fix_exec_bits(root: &Path) {
    // Ensure the launcher, the crash reporter and the helper binaries run.
    let mut stack = vec![root.to_path_buf()];
    use std::os::unix::fs::PermissionsExt;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mut perms = meta.permissions();
                if perms.mode() & 0o111 == 0 {
                    perms.set_mode(perms.mode() | 0o111);
                    let _ = std::fs::set_permissions(&path, perms);
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn fix_exec_bits(_root: &Path) {}

/// Keep only the current version dir around.
fn prune_old_versions(dir: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("pending-") || (name != keep && name.contains('.')) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn last_check(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("LAST_UPDATE_CHECK"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn stamp_check(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join("LAST_UPDATE_CHECK"), now_millis().to_string());
}

/// Ensure a usable Firefox binary: installed, else download the newest
/// release of the channel and walk down until one passes the smoke test.
pub fn ensure_firefox() -> Result<PathBuf, String> {
    if let Some((_, bin)) = installed() {
        return Ok(bin);
    }
    if !autoupdate_enabled() {
        return Err("no Firefox installed (auto-update disabled)".into());
    }
    let channel = Channel::from_env();
    let newest = fetch_version(channel)?;
    for (step, version) in version_ladder(&newest).into_iter().enumerate() {
        match download_version(&version) {
            Ok(bin) => {
                stamp_check(&managed_dir());
                return Ok(bin);
            }
            Err(e) => {
                eprintln!("tontoo-webengine: Firefox {version} unusable ({e})");
            }
        }
        if step >= MAX_LADDER_STEPS {
            break;
        }
    }
    Err(format!("no runnable Firefox build found (newest {newest})"))
}

/// Background update check at helper startup.
///
/// Two jobs, both off the critical path:
///
/// 1. Nothing installed yet: provision the newest build of the channel.
///    The caller keeps running on the mock renderer meanwhile; the new
///    browser activates on the next start.
/// 2. A build is installed: refresh it when a newer release exists and
///    the last check is older than [`UPDATE_INTERVAL`]. The running build
///    keeps serving; the new one activates on the next start.
///
/// Skipped when network provisioning is disabled.
pub fn maybe_background_update() {
    let dir = managed_dir();
    if !autoupdate_enabled() {
        return;
    }
    let Some(installed_version) = installed().map(|(v, _)| v) else {
        std::thread::spawn(move || match ensure_firefox() {
            Ok(bin) => eprintln!(
                "tontoo-webengine: provisioned {} (active on next start)",
                bin.display()
            ),
            Err(e) => eprintln!("tontoo-webengine: no Firefox to provision ({e})"),
        });
        return;
    };
    if now_millis().saturating_sub(last_check(&dir)) < UPDATE_INTERVAL.as_millis() as u64 {
        return;
    }
    std::thread::spawn(move || {
        stamp_check(&dir);
        let channel = Channel::from_env();
        let Ok(newest) = fetch_version(channel) else {
            return;
        };
        if newest == installed_version {
            return;
        }
        eprintln!("tontoo-webengine: updating Firefox to {newest} in the background...");
        match download_version(&newest) {
            Ok(_) => {
                eprintln!("tontoo-webengine: Firefox updated to {newest} (active on next start)")
            }
            Err(e) => eprintln!("tontoo-webengine: background update failed ({e})"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing() {
        let json = foundation::serialization::JsonValue::parse(
            r#"{"FIREFOX_ESR":"140.17.0esr","LATEST_FIREFOX_VERSION":"157.0"}"#,
        )
        .unwrap();
        assert_eq!(
            parse_version(&json, "FIREFOX_ESR").as_deref(),
            Some("140.17.0esr")
        );
        assert_eq!(
            parse_version(&json, "LATEST_FIREFOX_VERSION").as_deref(),
            Some("157.0")
        );
        assert!(parse_version(&json, "MISSING").is_none());
    }

    #[test]
    fn ladder_steps_down_minors() {
        let ladder = version_ladder("140.17.0esr");
        assert_eq!(ladder[0], "140.17.0esr");
        assert_eq!(ladder[1], "140.16.0esr");
        // Bounded walk: at most MAX_LADDER_STEPS candidates.
        assert_eq!(ladder.len(), MAX_LADDER_STEPS);
        assert_eq!(ladder.last().unwrap(), "140.10.0esr");
        // A two-component version has no minor to step down.
        assert_eq!(version_ladder("157.0"), vec!["157.0".to_string()]);
        // Unparsable versions are returned unchanged.
        assert_eq!(version_ladder("nightly"), vec!["nightly".to_string()]);
    }

    #[test]
    fn urls_follow_the_mozilla_layout() {
        let urls = download_urls("140.17.0esr", "linux-x86_64");
        assert_eq!(
            urls[0],
            format!("{RELEASE_BASE}/140.17.0esr/linux-x86_64/en-US/firefox-140.17.0esr.tar.xz")
        );
        assert!(urls[1].ends_with(".tar.bz2"));
    }

    #[test]
    fn smoke_rejects_missing_binary() {
        assert!(!smoke_test(Path::new("/nonexistent/firefox-test-binary")));
    }

    #[test]
    fn managed_dir_override() {
        std::env::set_var("TONTOO_FIREFOX_DIR", "/tmp/tontoo-gecko-dir-test");
        assert_eq!(managed_dir(), PathBuf::from("/tmp/tontoo-gecko-dir-test"));
        std::env::remove_var("TONTOO_FIREFOX_DIR");
        assert!(managed_dir().to_string_lossy().contains("tontoo-webengine"));
    }

    #[test]
    fn provision_flag_and_channel() {
        std::env::set_var("TONTOO_FIREFOX_AUTOUPDATE", "0");
        assert!(!autoupdate_enabled());
        std::env::remove_var("TONTOO_FIREFOX_AUTOUPDATE");
        assert!(autoupdate_enabled());
        std::env::set_var("TONTOO_FIREFOX_CHANNEL", "release");
        assert_eq!(Channel::from_env(), Channel::Release);
        std::env::remove_var("TONTOO_FIREFOX_CHANNEL");
        assert_eq!(Channel::from_env(), Channel::Esr);
    }
}
