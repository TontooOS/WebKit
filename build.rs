//! Build script for the WPE WebKit offscreen backend (feature `wpe`).
//!
//! The C shim in `src/wpe/tontoo_wpe.c` talks to WPE WebKit, libwpe, the
//! FDO backend and EGL. Every dependency is resolved through `pkg-config`
//! so the crate builds against whatever Arch ships:
//!
//! | Package | Provides |
//! |---|---|
//! | `wpewebkit` | `wpe-webkit-2.0.pc`, the WebKit C API |
//! | `libwpe` | `wpe-1.0.pc`, the embedder API |
//! | `wpebackend-fdo` | `wpebackend-fdo-1.0.pc`, the offscreen exportable |
//! | `egl` | surfaceless EGL display |
//! | `wayland-server` | `wl_shm_buffer` accessors for the frames |
//!
//! Nothing else is needed: no GTK, no display, no network at build time.
//!
//! When the shim cannot be built (missing packages, no compiler) the
//! script emits `tontoo_wpe_missing` and `src/wpe.rs` keeps the other
//! engines working.

use std::process::Command;

/// Packages probed through `pkg-config`, in link order.
const PACKAGES: [&str; 7] = [
    "wpe-webkit-2.0",
    "wpe-1.0",
    "wpebackend-fdo-1.0",
    "glib-2.0",
    "gobject-2.0",
    "egl",
    "wayland-server",
];

/// Libraries the shim needs, in link order. Filtered by what pkg-config
/// actually reports so a partially installed stack still links.
const LIBS: [&str; 11] = [
    "WPEWebKit-2.0",
    "WPEBackend-fdo-1.0",
    "wpe-1.0",
    "EGL",
    "wayland-server",
    "soup-3.0",
    "gio-2.0",
    "gobject-2.0",
    "glib-2.0",
    "gmodule-2.0",
    "m",
];

/// Runs `pkg-config` through `/bin/sh`.
///
/// Cargo runs build scripts with a descriptor setup where spawning
/// `pkg-config` directly makes `pkgconf` exit with status 1 and no output,
/// while the same command inside a shell resolves the packages fine. The
/// package list is a compile-time constant, so building the command line is
/// safe here.
fn pkg_config(flag: &str) -> Option<String> {
    let command = format!("exec pkg-config --{flag} {}", PACKAGES.join(" "));
    let output = Command::new("/bin/sh").arg("-c").arg(&command).output().ok()?;
    if !output.status.success() {
        println!(
            "cargo:warning=tontoo-wpe: `{}` failed: {}",
            command,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_WPE").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=src/wpe/tontoo_wpe.c");
    println!("cargo:rerun-if-changed=src/wpe/tontoo_wpe.h");

    // Include paths have to be known *before* the shim is compiled, so the
    // pkg-config query runs first.
    let cflags = pkg_config("cflags").unwrap_or_default();
    let mut shim = cc::Build::new();
    shim
        .file("src/wpe/tontoo_wpe.c")
        .include("src/wpe")
        .flag_if_supported("-std=gnu11")
        .warnings(false);
    for path in cflags.split_whitespace() {
        if let Some(dir) = path.strip_prefix("-I") {
            shim.include(dir);
        }
    }
    if shim.try_compile("tontoo_wpe").is_err() {
        println!("cargo:warning=tontoo-wpe: the WPE C shim failed to build; \
                  install wpewebkit, libwpe, wpebackend-fdo, mesa and try again");
        println!("cargo:rustc-cfg=tontoo_wpe_missing");
        return;
    }

    for path in cflags.split_whitespace() {
        if let Some(dir) = path.strip_prefix("-I") {
            println!("cargo:include={dir}");
        }
    }
    let libs = pkg_config("libs").unwrap_or_default();
    let mut linked_any = false;
    for lib in LIBS {
        if libs.contains(&format!("-l{lib}")) {
            println!("cargo:rustc-link-lib=dylib={lib}");
            linked_any = true;
        }
    }
    if !linked_any {
        println!("cargo:warning=tontoo-wpe: pkg-config reported no libraries");
        println!("cargo:rustc-cfg=tontoo_wpe_missing");
    }
}
