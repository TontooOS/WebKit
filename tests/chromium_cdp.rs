//! Chromium end-to-end test (needs helper binary + system Chromium).
//!
//! Run with: `cargo test --test chromium_cdp` (the helper is auto-built
//! with `cargo test`). Skips trivially when no Chromium binary exists.

#![cfg(feature = "chromium")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use webkit::{ProcessEngine, WebEngine, WebKitConfiguration, WebView};

struct FailureProbe {
    failure: Rc<RefCell<Option<String>>>,
}

impl webkit::WebViewDelegate for FailureProbe {
    fn load_failed(&mut self, _url: Option<&str>, error: &str) {
        *self.failure.borrow_mut() = Some(error.to_string());
    }
}

#[test]
fn chromium_end_to_end() {
    let helper = option_env!("CARGO_BIN_EXE_tontoo-webengine");
    let Some(helper) = helper else {
        eprintln!("helper binary unknown, skipping chromium e2e");
        return;
    };
    if !std::path::Path::new(helper).is_file() {
        eprintln!("helper binary missing, skipping chromium e2e");
        return;
    }
    if webkit::chromium::find_chrome().is_err() {
        eprintln!("no Chromium binary, skipping chromium e2e (set CHROMIUM_BIN)");
        return;
    }
    // Force the chromium renderer: without a Chromium binary the helper
    // reports unavailability and this test skips.
    std::env::set_var("TONTOO_WEBENGINE_RENDERER", "chromium");
    let engine = match ProcessEngine::spawn_with_bin(std::path::Path::new(helper)) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("helper spawn failed, skipping chromium e2e ({e})");
            return;
        }
    };
    let engine: Arc<dyn WebEngine> = Arc::new(engine);
    let view = WebView::with_engine(WebKitConfiguration::new(), engine).unwrap();
    let failure = Rc::new(RefCell::new(None));
    view.set_delegate(Box::new(FailureProbe {
        failure: failure.clone(),
    }));
    view.load_url("data:text/html,<title>Hello</title><h1>Hi</h1>")
        .unwrap();

    // Wait for the first real frame. Cold browsers on weak machines need
    // minutes; override with TONTOO_E2E_TIMEOUT_SECS (default 240).
    let budget = std::env::var("TONTOO_E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(240);
    let deadline = Instant::now() + Duration::from_secs(budget);
    let mut pumps = 0u32;
    let frame = loop {
        view.pump_events();
        pumps += 1;
        if pumps % 100 == 0 {
            eprintln!("chromium e2e: still waiting for first frame ({pumps} pumps)");
        }
        if let Some(failure) = failure.borrow().clone() {
            if failure.contains("chromium unavailable") {
                eprintln!("no Chromium binary, skipping chromium e2e");
                return;
            }
            panic!("unexpected load failure: {failure}");
        }
        if let Some(frame) = view.poll_frame() {
            // Mock frames are 800x600 checkerboards too; require the
            // chromium renderer line via a real JS answer below.
            break frame;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for first frame");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        frame.rgba.len(),
        frame.width as usize * frame.height as usize * 4
    );

    // Real JavaScript inside the Chromium sandbox.
    let result = view.evaluate_javascript("1+1").unwrap();
    assert_eq!(result, serde_json::json!(2));
    let title = view.evaluate_javascript("document.title").unwrap();
    assert_eq!(title, serde_json::json!("Hello"));
}
