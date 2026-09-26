//! Self-provisioning Chrome-for-Testing builds (feature `chromium`).
//!
//! When no usable Chromium binary exists, the helper downloads one: newest
//! Stable first, then older milestones until a build passes the smoke test.
//! The smoke test (not a hardcoded table) decides, so old glibc systems
//! like WSL containers automatically land on a runnable build while modern
//! systems track the latest Stable with its security fixes.
//!
//! Layout (per user, shared by all helper instances):
//!
//! ```text
//! <managed>/VERSION              installed version, e.g. 154.0.8037.57
//! <managed>/<version>/...        extracted CfT archive (chrome-linux64/…)
//! <managed>/LAST_UPDATE_CHECK    unix millis of the last update poll
//! <managed>/known-good.json      cached milestone list (24 h TTL)
//! ```
//!
//! Env: `TONTOO_CHROME_DIR` overrides the managed dir,
//! `TONTOO_CHROME_AUTOUPDATE=0` disables all network provisioning.

#![cfg(feature = "chromium")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Small CfT status document with the current channels.
pub const CFT_LAST_KNOWN_GOOD: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions.json";
/// Full milestone list with per-platform download URLs (large, cached).
pub const CFT_KNOWN_GOOD: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json";

const UPDATE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// How many older majors to try before giving up.
const MAX_LADDER_STEPS: usize = 24;

/// Network provisioning allowed unless explicitly disabled.
pub fn autoupdate_enabled() -> bool {
    std::env::var("TONTOO_CHROME_AUTOUPDATE").as_deref() != Ok("0")
}

/// Managed install dir: override, Windows LocalAppData, or XDG data home.
pub fn managed_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TONTOO_CHROME_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local)
                .join("TontooWebEngine")
                .join("chrome");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    PathBuf::from(base).join("tontoo-webengine").join("chrome")
}

/// CfT platform slug for this host.
pub fn platform_slug() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", _) => "linux64",
        ("windows", _) => "win64",
        ("macos", "aarch64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        _ => "linux64",
    }
}

/// Binary path inside an extracted CfT archive for this host.
pub fn binary_relpath() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", _) => "chrome-win64/chrome.exe",
        ("macos", "aarch64") => {
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        }
        ("macos", _) => {
            "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        }
        _ => "chrome-linux64/chrome",
    }
}

/// Installed version from the VERSION file, if the binary still exists.
pub fn installed() -> Option<(String, PathBuf)> {
    let dir = managed_dir();
    let version = std::fs::read_to_string(dir.join("VERSION")).ok()?;
    let version = version.trim().to_string();
    if version.is_empty() {
        return None;
    }
    let bin = dir.join(&version).join(binary_relpath());
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

/// GET a URL as text.
fn http_get_text(url: &str) -> Result<String, String> {
    let agent = ureq_agent()?;
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| e.to_string())
        .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
}

fn ureq_agent() -> Result<ureq::Agent, String> {
    // ureq 3 builder API; falls back to a default agent when the builder
    // shape differs (kept compiling across ureq minor versions).
    Ok(build_agent())
}

/// Short-timeout agent for small JSON documents.
#[allow(clippy::all)]
fn build_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

/// Long-timeout agent for archive downloads (hundreds of MB).
fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30 * 60)))
        .build()
        .into()
}

/// Parse the Stable version out of last-known-good-versions.json.
pub(crate) fn parse_last_known_good(json: &serde_json::Value) -> Option<String> {
    json.get("channels")?
        .get("Stable")?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

/// Fetch the current Stable version.
pub fn fetch_stable_version() -> Result<String, String> {
    let text = http_get_text(CFT_LAST_KNOWN_GOOD)?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("CfT json: {e}"))?;
    parse_last_known_good(&json).ok_or_else(|| "CfT json without Stable".into())
}

/// Version ladder: newest-first full versions per major that ship a build
/// for this host, from known-good-versions-with-downloads.json.
pub(crate) fn ladder(json: &serde_json::Value, platform: &str) -> Vec<String> {
    use std::collections::BTreeMap;
    // major -> newest full version seen.
    let mut majors: BTreeMap<u64, String> = BTreeMap::new();
    if let Some(versions) = json.get("versions").and_then(|v| v.as_array()) {
        for entry in versions {
            let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(major) = version.split('.').next().and_then(|m| m.parse::<u64>().ok()) else {
                continue;
            };
            let has_platform = entry
                .get("downloads")
                .and_then(|d| d.get("chrome"))
                .and_then(|c| c.as_array())
                .map(|items| {
                    items.iter().any(|item| {
                        item.get("platform").and_then(|p| p.as_str()) == Some(platform)
                    })
                })
                .unwrap_or(false);
            if !has_platform {
                continue;
            }
            match majors.get(&major) {
                Some(known) if version_le(known, version) => {
                    majors.insert(major, version.to_string());
                }
                None => {
                    majors.insert(major, version.to_string());
                }
                _ => {}
            }
        }
    }
    majors.into_values().rev().collect()
}

/// Numeric dotted-version comparison.
fn version_le(a: &str, b: &str) -> bool {
    version_parts(a) <= version_parts(b)
}

fn version_parts(v: &str) -> Vec<u64> {
    v.split('.').filter_map(|p| p.parse::<u64>().ok()).collect()
}

/// Download a CfT chrome archive and unpack it into the managed dir.
/// Returns the binary path on success (smoke-tested).
fn download_version(version: &str) -> Result<PathBuf, String> {
    let dir = managed_dir();
    let platform = platform_slug();
    let url = format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{version}/{platform}/chrome-{platform}.zip"
    );
    eprintln!("tontoo-webengine: downloading Chromium {version} ({platform})...");
    let mut response = download_agent()
        .get(&url)
        .call()
        .map_err(|e| format!("download: {e}"))?;
    // Default body cap is 10 MB; archives are hundreds of MB.
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 << 30)
        .read_to_vec()
        .map_err(|e| format!("download: {e}"))?;
    eprintln!(
        "tontoo-webengine: downloaded {} MB, unpacking...",
        bytes.len() / 1_048_576
    );

    let pending = dir.join(format!("pending-{version}"));
    let _ = std::fs::remove_dir_all(&pending);
    std::fs::create_dir_all(&pending).map_err(|e| e.to_string())?;
    let zip_path = pending.join("chrome.zip");
    std::fs::write(&zip_path, &bytes).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("zip: {e}"))?;
    archive
        .extract(&pending)
        .map_err(|e| format!("unzip: {e}"))?;
    let _ = std::fs::remove_file(&zip_path);
    fix_exec_bits(&pending);

    let bin = pending.join(binary_relpath());
    if !bin.is_file() {
        let _ = std::fs::remove_dir_all(&pending);
        return Err(format!("archive without binary for {platform}"));
    }
    if !smoke_test(&bin) {
        let _ = std::fs::remove_dir_all(&pending);
        return Err(format!("Chromium {version} does not run here"));
    }
    let target = dir.join(version);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&pending, &target).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("VERSION"), version).map_err(|e| e.to_string())?;
    prune_old_versions(&dir, version);
    Ok(target.join(binary_relpath()))
}

#[cfg(unix)]
fn fix_exec_bits(root: &Path) {
    // CfT zips usually preserve modes; ensure the launcher files run.
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
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("chrome")
                || name.starts_with("crashpad")
                || name.ends_with(".sh")
            {
                if let Ok(meta) = entry.metadata() {
                    let mut perms = meta.permissions();
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
        if name.starts_with("pending-") || (name != keep && version_parts(&name).len() >= 2) {
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

/// Cached milestone list (24 h TTL), refreshed on demand.
fn known_good_versions() -> Result<serde_json::Value, String> {
    let dir = managed_dir();
    let cache = dir.join("known-good.json");
    let fresh = std::fs::metadata(&cache)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|age| age < UPDATE_INTERVAL)
        .unwrap_or(false);
    if fresh {
        if let Ok(text) = std::fs::read_to_string(&cache) {
            if let Ok(json) = serde_json::from_str(&text) {
                return Ok(json);
            }
        }
    }
    let text = http_get_text(CFT_KNOWN_GOOD)?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("CfT json: {e}"))?;
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&cache, &text);
    Ok(json)
}

/// Ensure a usable Chromium binary: installed, else download newest-first
/// until one passes the smoke test.
pub fn ensure_chrome() -> Result<PathBuf, String> {
    if let Some((_, bin)) = installed() {
        return Ok(bin);
    }
    if !autoupdate_enabled() {
        return Err("no Chromium installed (auto-update disabled)".into());
    }
    // Fast path: current Stable.
    let stable = fetch_stable_version().ok();
    if let Some(stable) = stable.clone() {
        match download_version(&stable) {
            Ok(bin) => {
                stamp_check(&managed_dir());
                return Ok(bin);
            }
            Err(e) => eprintln!("tontoo-webengine: Stable {stable} unusable ({e}), trying older..."),
        }
    }
    // Ladder: newest version per older major, never newer than Stable
    // (skips Canary/Beta above the release channel).
    let json = known_good_versions()?;
    let platform = platform_slug();
    let mut steps = 0;
    for version in ladder(&json, platform) {
        if let Some(stable) = stable.as_deref() {
            if version_parts(&version) > version_parts(stable) {
                continue;
            }
        }
        if installed().map(|(v, _)| v == version).unwrap_or(false) {
            continue;
        }
        match download_version(&version) {
            Ok(bin) => {
                stamp_check(&managed_dir());
                return Ok(bin);
            }
            Err(e) => eprintln!("tontoo-webengine: {version} unusable ({e})"),
        }
        steps += 1;
        if steps >= MAX_LADDER_STEPS {
            break;
        }
    }
    Err("no runnable Chromium build found".into())
}

/// Background update check at helper startup: refresh the installed build
/// when a newer Stable exists. Never blocks the caller. Skipped when
/// nothing is installed yet (the synchronous provision path owns that).
pub fn maybe_background_update() {
    let dir = managed_dir();
    let installed_version = installed().map(|(v, _)| v);
    if installed_version.is_none() || !autoupdate_enabled() {
        return;
    }
    if now_millis().saturating_sub(last_check(&dir)) < UPDATE_INTERVAL.as_millis() as u64 {
        return;
    }
    std::thread::spawn(move || {
        stamp_check(&dir);
        let Ok(stable) = fetch_stable_version() else {
            return;
        };
        if installed_version.as_deref() == Some(stable.as_str()) {
            return;
        }
        eprintln!("tontoo-webengine: updating Chromium to {stable} in the background...");
        match download_version(&stable) {
            Ok(_) => eprintln!("tontoo-webengine: Chromium updated to {stable} (active on next start)"),
            Err(e) => eprintln!("tontoo-webengine: background update failed ({e})"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_parsing() {
        let json = serde_json::json!({
            "channels": { "Stable": { "version": "154.0.8037.57" } }
        });
        assert_eq!(parse_last_known_good(&json).as_deref(), Some("154.0.8037.57"));
        assert!(parse_last_known_good(&serde_json::json!({})).is_none());
    }

    #[test]
    fn ladder_orders_majors_desc() {
        let json = serde_json::json!({
            "versions": [
                { "version": "154.0.1.0", "downloads": { "chrome": [{ "platform": "linux64", "url": "u" }] } },
                { "version": "154.0.0.5", "downloads": { "chrome": [{ "platform": "linux64", "url": "u" }] } },
                { "version": "153.2.0.0", "downloads": { "chrome": [{ "platform": "win64", "url": "u" }] } },
                { "version": "152.9.9.9", "downloads": { "chrome": [{ "platform": "linux64", "url": "u" }] } },
                { "version": "notaversion", "downloads": {} },
            ]
        });
        // 153 lacks linux64, so the ladder is 154 then 152.
        assert_eq!(ladder(&json, "linux64"), vec!["154.0.1.0", "152.9.9.9"]);
    }

    #[test]
    fn version_ordering() {
        assert!(version_le("154.0.1.0", "154.0.8037.57"));
        assert!(version_le("154.0.8037.57", "154.0.8037.57"));
        assert!(!version_le("155.0.0.0", "154.9.9.9"));
    }

    #[test]
    fn smoke_rejects_missing_binary() {
        assert!(!smoke_test(Path::new("/nonexistent/chrome-test-binary")));
    }

    #[test]
    fn managed_dir_override() {
        std::env::set_var("TONTOO_CHROME_DIR", "/tmp/tontoo-chrome-dir-test");
        assert_eq!(managed_dir(), PathBuf::from("/tmp/tontoo-chrome-dir-test"));
        std::env::remove_var("TONTOO_CHROME_DIR");
        assert!(managed_dir().to_string_lossy().contains("tontoo-webengine"));
    }

    #[test]
    fn provision_flag() {
        std::env::set_var("TONTOO_CHROME_AUTOUPDATE", "0");
        assert!(!autoupdate_enabled());
        std::env::remove_var("TONTOO_CHROME_AUTOUPDATE");
        assert!(autoupdate_enabled());
    }
}
