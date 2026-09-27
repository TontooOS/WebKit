//! Helper round-trip test (needs a built `tontoo-webengine` and the
//! `vello` feature).
//!
//! Run with:
//! `TONTOO_WEBENGINE_BIN=../../target/debug/tontoo-webengine cargo test
//! --test process`. Without the env var the test passes trivially.

#![cfg(feature = "vello")]

use std::sync::Arc;

use webkit::{ProcessEngine, WebEngine, WebKitConfiguration, WebView};

#[test]
fn helper_roundtrip() {
    let bin = std::env::var("TONTOO_WEBENGINE_BIN").unwrap_or_default();
    if bin.is_empty() {
        eprintln!("TONTOO_WEBENGINE_BIN unset, skipping helper round-trip");
        return;
    }
    let engine = ProcessEngine::spawn_with_bin(std::path::Path::new(&bin))
        .expect("spawn helper");
    let engine: Arc<dyn WebEngine> = Arc::new(engine);
    let view = WebView::with_engine(
        WebKitConfiguration::new().start_url("https://example.com"),
        engine,
    )
    .expect("build view");

    // Wait for the first frame (up to 5 seconds).
    let frame = (0..500)
        .filter_map(|_| {
            view.pump_events();
            let frame = view.poll_frame();
            if frame.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            frame
        })
        .next()
        .expect("helper serves a frame");
    assert_eq!(frame.rgba.len(), frame.width as usize * frame.height as usize * 4);

    // Load lifecycle events arrive through the queue.
    view.pump_events();
    assert_eq!(view.url().as_deref(), Some("https://example.com"));
    assert!(view.title().is_some());

    // JavaScript round-trips through the id correlation.
    let result = view.evaluate_javascript("document.title").unwrap();
    assert_eq!(result, foundation::serialization::JsonValue::Null);

    // Cookie store round-trips.
    let cookie = webkit::Cookie::new("session", "abc", "example.com");
    view.add_cookie(&cookie);
    let cookies = view.list_cookies().unwrap();
    assert_eq!(cookies.len(), 1);
    assert_eq!(cookies[0].value, "abc");
    view.delete_cookie("example.com", "/", "session");
    assert!(view.list_cookies().unwrap().is_empty());
}
