//! Diagnostic: does the page's animation clock run, and does WebKit export a
//! frame for every repaint?
//!
//! Run with: `cargo test --release --no-default-features --features wpe --test wpe_diag -- --nocapture`

#![cfg(feature = "wpe")]

use std::time::{Duration, Instant};

use webkit::engine::{EngineCommand, EngineEvent, WebEngine};
use webkit::wpe::{is_available, WpeEngine, WpeOptions};

const ANIMATED_PAGE: &str = "data:text/html,<body style='margin:0;background:%23112233'>\
<style>@keyframes move{from{transform:translateX(0)}to{transform:translateX(600px)}}</style>\
<div id='box' style='width:120px;height:120px;background:%23ff8800;\
animation:move 2s linear infinite'></div>";

#[test]
fn animation_clock_runs() {
    if !is_available() {
        eprintln!("skipping: WPE unavailable");
        return;
    }
    let engine = WpeEngine::launch(WpeOptions {
        width: 800,
        height: 600,
        start_url: Some(ANIMATED_PAGE.to_string()),
        ..WpeOptions::default()
    })
    .expect("engine starts");

    let start = Instant::now();
    let mut next_id = 1u64;
    let mut js_results: Vec<String> = Vec::new();
    let mut painted = 0u32;
    let mut last_seq = 0u64;

    // Sample the animation clock twice, two seconds apart.
    let mut sample_at: Option<Instant> = None;
    while start.elapsed() < Duration::from_secs(8) {
        if sample_at.is_none() {
            sample_at = Some(start + Duration::from_secs(3));
            engine.send(EngineCommand::EvaluateJs {
                id: next_id,
                script: "JSON.stringify({t: document.getAnimations().map(a=>Math.round(a.currentTime)), x: getComputedStyle(document.getElementById('box')).transform})".into(),
            });
            next_id += 1;
        } else if start.elapsed() >= Duration::from_secs(6) && js_results.len() < 2 {
            engine.send(EngineCommand::EvaluateJs {
                id: next_id,
                script: "JSON.stringify({t: document.getAnimations().map(a=>Math.round(a.currentTime)), x: getComputedStyle(document.getElementById('box')).transform})".into(),
            });
            next_id += 1;
        }
        for event in engine.drain_events() {
            if let EngineEvent::JsResult { id, result } = event {
                eprintln!("[diag] js#{id} -> {}", result.to_string());
                js_results.push(id.to_string());
            }
        }
        if let Some(frame) = engine.latest_frame() {
            if frame.seq != last_seq {
                last_seq = frame.seq;
                painted += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("[diag] painted frames = {painted}, js samples = {}", js_results.len());
}