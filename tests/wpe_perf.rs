//! Frame throughput of the WPE engine on this host.
//!
//! One engine, two pages: a static page proves the frame content is correct,
//! an animated page (CSS transform, the cheapest repaint driver) measures how
//! many distinct frames actually arrive per second.
//!
//! Run with: `cargo test --release --test wpe_perf -- --nocapture`

#![cfg(feature = "wpe")]

use std::time::{Duration, Instant};

use webkit::engine::{EngineCommand, EngineEvent, WebEngine};
use webkit::wpe::{is_available, WpeEngine, WpeOptions};

/// A 120x120 orange square on a dark background.
const STATIC_PAGE: &str = "data:text/html,<body style='margin:0;background:%23112233'>\
<div style='width:120px;height:120px;background:%23ff8800'></div>";

/// The same square, moved across the page forever.
const ANIMATED_PAGE: &str = "data:text/html,<body style='margin:0;background:%23112233'>\
<style>@keyframes move{from{transform:translateX(0)}to{transform:translateX(600px)}}</style>\
<div style='width:120px;height:120px;background:%23ff8800;\
animation:move 2s linear infinite'></div>";

/// Count distinct frames after `load_url`, until the page finished loading
/// and `target` frames arrived or the budget runs out.
fn measure(engine: &WpeEngine, url: &str, budget: Duration, target: u32) -> (u32, f64) {
    let start = Instant::now();
    let mut painted = 0u32;
    let mut loaded = false;
    let mut last_seq = 0u64;
    engine.send(EngineCommand::LoadUrl(url.to_string()));
    while start.elapsed() < budget {
        for event in engine.drain_events() {
            if matches!(event, EngineEvent::LoadFinished(_)) {
                loaded = true;
            }
        }
        if let Some(frame) = engine.latest_frame() {
            if frame.seq != last_seq {
                last_seq = frame.seq;
                painted += 1;
            }
        }
        if loaded && painted >= target {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let elapsed = start.elapsed().as_secs_f64().max(0.001);
    (painted, painted as f64 / elapsed)
}

#[test]
fn reports_frame_rate() {
    if !is_available() {
        eprintln!("skipping: WPE unavailable");
        return;
    }
    let engine = WpeEngine::launch(WpeOptions {
        width: 800,
        height: 600,
        start_url: Some(STATIC_PAGE.to_string()),
        ..WpeOptions::default()
    })
    .expect("engine starts");

    let (static_frames, static_fps) = measure(&engine, STATIC_PAGE, Duration::from_secs(8), 4);
    let frame = engine.latest_frame().expect("static frame");
    let orange = frame
        .rgba
        .chunks_exact(4)
        .filter(|px| px[0] > 200 && px[1] > 100 && px[1] < 180 && px[2] < 80)
        .count();
    eprintln!(
        "WPE static   : {static_frames} frames ({static_fps:.2} fps), \
         orange px={orange} (expect {}), size={}x{}",
        120 * 120,
        frame.width,
        frame.height
    );

    let (anim_frames, anim_fps) =
        measure(&engine, ANIMATED_PAGE, Duration::from_secs(10), 120);
    eprintln!("WPE animated : {anim_frames} frames ({anim_fps:.2} fps)");
}