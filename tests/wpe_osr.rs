//! End-to-end test for the offscreen WPE WebKit backend.
//!
//! Loads a page, waits for the load to finish and then checks that the engine
//! produced a real BGRA frame with the requested geometry. It needs
//! `wpewebkit`, `libwpe`, `wpebackend-fdo` and a surfaceless EGL driver, but
//! no display server and no network access.

#![cfg(feature = "wpe")]

use std::time::{Duration, Instant};

use webkit::engine::{EngineEvent, SharedFrame, WebEngine};
use webkit::wpe::{is_available, WpeEngine, WpeOptions};

const PAGE: &str = "data:text/html,<body style='margin:0;background:%2300ff00'><h1>tontoo</h1></body>";

/// Whether a frame shows the green page background. The buffer is BGRA, so
/// green is the middle byte.
fn is_green(frame: &SharedFrame) -> bool {
    frame
        .rgba
        .chunks_exact(4)
        .filter(|px| px[1] > 200 && px[0] < 60 && px[2] < 60)
        .count()
        > frame.rgba.len() / 4 / 100
}

/// Poll `check` every 16 ms until it returns true or the deadline passes.
fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    false
}

#[test]
fn renders_a_frame_offscreen() {
    if !is_available() {
        eprintln!("skipping: WPE shim or its libraries are not available");
        return;
    }

    let engine = WpeEngine::launch(WpeOptions {
        width: 640,
        height: 480,
        start_url: Some(PAGE.to_string()),
        ..WpeOptions::default()
    })
    .expect("wpe engine starts");

    let mut loaded = false;
    let mut frame = None;
    // The first frame can still be the unpainted surface, so wait for a frame
    // that actually contains the page instead of just any frame.
    assert!(
        wait_until(Duration::from_secs(30), || {
            for event in engine.drain_events() {
                if matches!(event, EngineEvent::LoadFinished(_)) {
                    loaded = true;
                }
            }
            let latest = engine.latest_frame();
            if let Some(candidate) = latest {
                if is_green(&candidate) {
                    frame = Some(candidate);
                }
            }
            loaded && frame.is_some()
        }),
        "wpe never reported a finished load together with a painted frame"
    );

    let frame = frame.expect("frame");
    assert_eq!((frame.width, frame.height), (640, 480));
    assert_eq!(frame.rgba.len(), 640 * 480 * 4);

    // The frame is BGRA, so green shows up as `px[1]`.
    let green = frame
        .rgba
        .chunks_exact(4)
        .filter(|px| px[1] > 200 && px[0] < 60 && px[2] < 60)
        .count();
    assert!(
        green > frame.rgba.len() / 4 / 100,
        "the green page background never reached the frame buffer: green={green} first_pixel={:?}",
        &frame.rgba[..4]
    );
}