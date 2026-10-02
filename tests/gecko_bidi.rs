//! Gecko end-to-end test.
//!
//! Ignored by default because it launches a real Firefox: the session
//! setup alone takes minutes on a software-rendered host, and the run is
//! timing-sensitive. Run it explicitly:
//!
//! ```bash
//! cargo test --test gecko_bidi -- --ignored --nocapture
//! ```
//!
//! With no Firefox installed the first run provisions one (set
//! `TONTOO_FIREFOX_AUTOUPDATE=0` to skip instead, or
//! `TONTOO_FIREFOX_BIN=/path/to/firefox` to use a specific build). The
//! test launches a headless, private Firefox, drives a real page over
//! WebDriver BiDi and round-trips JavaScript results, the cookie jar and
//! the WebExtension module. `TONTOO_E2E_TIMEOUT_SECS` sets the per-step
//! budget (default 90 s; raise it on slow hosts).
//!
//! The test drives [`webkit::GeckoEngine`] directly, so it needs only the
//! `gecko` feature and no TontooUI.
#![cfg(feature = "gecko")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use foundation::serialization::JsonValue;
use webkit::{Cookie, EngineCommand, EngineEvent, GeckoEngine, GeckoOptions, WebEngine};

/// Budget for one awaited engine event. Slow containers (software
/// rendering, cold profile) need minutes for the first BiDi command, so
/// `TONTOO_E2E_TIMEOUT_SECS` overrides it.
fn step_timeout() -> Duration {
    let secs = std::env::var("TONTOO_E2E_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(90);
    Duration::from_secs(secs)
}

/// Budget for the initial launch (cold start is slow in CI).
fn launch_timeout() -> Duration {
    step_timeout() * 3
}

fn download_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tontoo-gecko-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("download dir");
    dir
}

/// A page with a title, one element and a cookie.
const HELLO_PAGE: &str = "<!doctype html><title>Hello</title><p id=x>hi</p>\
     <script>document.cookie='tontoo=42; path=/';</script>";

/// A driven Firefox plus every engine event seen so far.
struct Session {
    engine: Arc<GeckoEngine>,
    events: Vec<EngineEvent>,
    timeout: Duration,
}

impl Session {
    fn new_with(engine: Arc<GeckoEngine>) -> Self {
        Self {
            engine,
            events: Vec::new(),
            timeout: step_timeout(),
        }
    }

    fn pump(&mut self) {
        self.events.extend(self.engine.drain_events());
    }

    /// Pump until `ready` sees an event, or the timeout expires.
    fn wait_for(
        &mut self,
        timeout: Duration,
        mut ready: impl FnMut(&EngineEvent) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if self.events.iter().any(|event| ready(event)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Evaluate JavaScript and return the correlated `JsResult` value.
    fn evaluate(&mut self, id: u64, script: &str) -> Option<JsonValue> {
        self.engine.send(EngineCommand::EvaluateJs {
            id,
            script: script.to_string(),
        });
        self.wait_for(self.timeout, |event| {
            matches!(event, EngineEvent::JsResult { id: got, .. } if *got == id)
        });
        self.events.iter().find_map(|event| match event {
            EngineEvent::JsResult { id: got, result } if *got == id => Some(result.clone()),
            _ => None,
        })
    }

    /// List cookies through the correlated `Cookies` event.
    fn cookies(&mut self, id: u64) -> Vec<Cookie> {
        self.engine.send(EngineCommand::ListCookies { id });
        self.wait_for(self.timeout, |event| {
            matches!(event, EngineEvent::Cookies { id: got, .. } if *got == id)
        });
        self.events
            .iter()
            .find_map(|event| match event {
                EngineEvent::Cookies { id: got, cookies } if *got == id => Some(cookies.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// URL of the last finished load.
    fn last_url(&self) -> Option<String> {
        self.events.iter().rev().find_map(|event| match event {
            EngineEvent::LoadFinished(url) => url.clone(),
            _ => None,
        })
    }

    fn title(&self) -> Option<String> {
        self.events.iter().rev().find_map(|event| match event {
            EngineEvent::Title(title) => title.clone(),
            _ => None,
        })
    }

    fn saw_load(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e, EngineEvent::LoadFinished(_)))
    }

    /// Compact dump of the events seen so far, for assertion messages.
    fn dump(&self) -> String {
        let names: Vec<&str> = self
            .events
            .iter()
            .map(|event| match event {
                EngineEvent::FrameReady { .. } => "FrameReady",
                EngineEvent::Title(_) => "Title",
                EngineEvent::Url(_) => "Url",
                EngineEvent::Progress(_) => "Progress",
                EngineEvent::LoadStarted(_) => "LoadStarted",
                EngineEvent::LoadFinished(_) => "LoadFinished",
                EngineEvent::LoadFailed { .. } => "LoadFailed",
                EngineEvent::ScriptMessage { .. } => "ScriptMessage",
                EngineEvent::JsResult { .. } => "JsResult",
                EngineEvent::ScriptDialog { .. } => "ScriptDialog",
                EngineEvent::PermissionRequest { .. } => "PermissionRequest",
                EngineEvent::DownloadStarted { .. } => "DownloadStarted",
                EngineEvent::DownloadProgress { .. } => "DownloadProgress",
                EngineEvent::DownloadFinished { .. } => "DownloadFinished",
                EngineEvent::DownloadFailed { .. } => "DownloadFailed",
                EngineEvent::Cookies { .. } => "Cookies",
                EngineEvent::History { .. } => "History",
                EngineEvent::EngineWindow { .. } => "EngineWindow",
                EngineEvent::ContextReady { .. } => "ContextReady",
                EngineEvent::ContextClosed { .. } => "ContextClosed",
                EngineEvent::ExtensionInstalled { .. } => "ExtensionInstalled",
                EngineEvent::ExtensionFailed { .. } => "ExtensionFailed",
                EngineEvent::Extensions { .. } => "Extensions",
                EngineEvent::ReadyToShow => "ReadyToShow",
            })
            .collect();
        format!("{names:?}")
    }
}

#[test]
#[ignore = "launches a real Firefox; run with cargo test --test gecko_bidi -- --ignored"]
fn gecko_end_to_end() {
    // Locate a browser. With no binary and provisioning enabled the test
    // downloads one (slow on the first run); with
    // TONTOO_FIREFOX_AUTOUPDATE=0 it skips instead.
    let provision = webkit::gecko_provision::autoupdate_enabled();
    if webkit::find_firefox().is_err() && !provision {
        eprintln!("no Firefox binary, skipping gecko e2e (set TONTOO_FIREFOX_BIN)");
        return;
    }

    let downloads = download_dir();
    let engine = match GeckoEngine::launch(GeckoOptions {
        kiosk: false,
        headless: true,
        private: true,
        download_dir: downloads.clone(),
        start_url: Some("about:blank".to_string()),
        provision,
        extensions: Vec::new(),
    }) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Firefox launch failed, skipping gecko e2e ({e})");
            return;
        }
    };
    let engine = Arc::new(engine);
    assert!(engine.pid() > 0, "Firefox must have a process id");

    let mut session = Session::new_with(engine);

    // The session reports the Firefox window as soon as it exists.
    assert!(
        session.wait_for(launch_timeout(), |event| {
            matches!(event, EngineEvent::EngineWindow { .. })
        }),
        "Firefox never reported an engine window: {}",
        session.dump()
    );

    // Navigate to a real page and wait for the load event.
    eprintln!("step: navigate to the data URL");
    let _ = session
        .engine
        .load_url(&webkit::gecko::html_data_url(HELLO_PAGE));
    session.wait_for(session.timeout, |event| {
        matches!(event, EngineEvent::LoadFinished(_))
    });
    assert!(
        session.saw_load(),
        "page never finished loading: {}",
        session.dump()
    );
    eprintln!("step: loaded {:?}", session.last_url());

    // Real JavaScript inside Gecko.
    eprintln!("step: evaluate 1 + 1");
    assert_eq!(
        session.evaluate(1, "1 + 1").and_then(|v| v.as_f64()),
        Some(2.0)
    );
    eprintln!("step: evaluate document.title");
    let title = session.evaluate(2, "document.title");
    eprintln!("step: document.title -> {title:?} events: {}", session.dump());
    assert_eq!(
        title.and_then(|v| match v {
            JsonValue::Str(text) => Some(text),
            _ => None,
        }),
        Some("Hello".into())
    );
    assert_eq!(
        session
            .evaluate(3, "document.getElementById('x').textContent")
            .and_then(|v| match v {
                JsonValue::Str(text) => Some(text),
                _ => None,
            }),
        Some("hi".into())
    );
    // The title arrives through the load event, not the evaluation.
    assert_eq!(session.title().as_deref(), Some("Hello"));

    // Cookie jar round-trip through the BiDi `storage` commands.
    // Note: a `data:` document has an opaque origin, so Firefox refuses
    // `document.cookie` there; the store itself is driven through the API.
    eprintln!("step: cookies");
    session.engine.send(EngineCommand::AddCookie {
        cookie: Cookie::new("added", "yes", "example.test")
            .secure(true)
            .expires(Some(4_102_444_800)),
    });
    let cookies = session.cookies(11);
    let added = cookies.iter().find(|c| c.name == "added");
    assert_eq!(
        added.map(|c| (c.value.as_str(), c.secure, c.domain.as_str())),
        Some(("yes", true, "example.test")),
        "added cookie missing or wrong: {cookies:?}"
    );
    assert_eq!(
        added.and_then(|c| c.expires),
        Some(4_102_444_800),
        "expiry did not round-trip: {cookies:?}"
    );
    session.engine.send(EngineCommand::DeleteCookie {
        domain: "example.test".into(),
        path: "/".into(),
        name: "added".into(),
    });
    assert!(
        !session.cookies(12).iter().any(|c| c.name == "added"),
        "deleted cookie still listed"
    );

    // WebExtension module. Builds without it answer with an error (or
    // never answer at all), so only a missing reply fails the test; the
    // supported startup path is `write_extension_policy`, unit-tested in
    // `gecko::tests::extension_policies_merge_into_the_profile`.
    eprintln!("step: webExtension.listExtensions");
    session.engine.send(EngineCommand::ListExtensions { id: 20 });
    let answered = session.wait_for(
        // Longer than the engine-side BiDi timeout (60 s), so a build
        // without the module still gets its failure event delivered.
        Duration::from_secs(90),
        |event| {
            matches!(event, EngineEvent::Extensions { id: 20, .. }
                | EngineEvent::ExtensionFailed { id: 20, .. })
        },
    );
    for event in &session.events {
        match event {
            EngineEvent::ExtensionFailed { id: 20, error } => {
                eprintln!("step: webExtension module answered with an error ({error})")
            }
            EngineEvent::Extensions { id: 20, extensions } => {
                eprintln!("step: installed extensions: {extensions:?}")
            }
            _ => {}
        }
    }
    assert!(
        answered,
        "webExtension.listExtensions neither answered nor failed: {}",
        session.dump()
    );

    // History navigation is driven through browsingContext.history.
    eprintln!("step: history");
    let before = session.events.len();
    session
        .engine
        .load_url("https://example.com/")
        .expect("valid URL");
    assert!(
        session.wait_for(session.timeout, |event| {
            matches!(event, EngineEvent::LoadFinished(Some(url)) if url.contains("example.com"))
        }),
        "example.com never finished loading: {}",
        session.dump()
    );
    session.events.truncate(before);
    session.engine.send(EngineCommand::GoBack);
    assert!(
        session.wait_for(session.timeout, |event| {
            matches!(event, EngineEvent::LoadFinished(_))
        }),
        "go_back never completed a load: {}",
        session.dump()
    );
    let back = session.last_url().unwrap_or_default();
    assert!(
        !back.contains("example.com"),
        "go_back stayed on the page: {back:?}"
    );

    let _ = std::fs::remove_dir_all(&downloads);
}
