//! TontooOS browser demo app built with UIKit + TontooWebKit.
//!
//! Shows how to embed a [`webkit::WebView`] in a UIKit app. The toolbar is
//! plain GTK4 so the buttons can capture the (non-`Send`) web view directly;
//! everything else follows the normal UIKit `App` / `AppDelegate` / `Widget`
//! flow.
//!
//! Run with: `cargo run --example browser`

use gtk::prelude::*;
use uikit::app::{App, AppDelegate, ColorScheme};
use uikit::widget::{apply_css, Widget, WidgetId};
use webkit::{lang, WebKitConfiguration, WebSettings, WebView, WebViewDelegate};

const START_URL: &str = "https://example.com";

/// Keep the web view handle and the status label shared with the toolbar.
struct BrowserState {
    web_view: std::rc::Rc<std::cell::RefCell<Option<WebView>>>,
    status: gtk::Label,
}

impl BrowserState {
    fn new() -> Self {
        Self {
            web_view: std::rc::Rc::new(std::cell::RefCell::new(None)),
            status: gtk::Label::new(None),
        }
    }
}

fn navigate(web_view: &std::rc::Rc<std::cell::RefCell<Option<WebView>>>, status: &gtk::Label, input: &str) {
    let url = normalize_url(input);
    match web_view.borrow().as_ref().map(|web| web.load_url(&url)) {
        Some(Ok(())) => {}
        Some(Err(e)) => status.set_text(&format!(
            "{}: {e}",
            lang::t_or("webkit.error", "Error")
        )),
        None => {}
    }
}

/// Turn a possibly scheme-less address into a loadable URL.
fn normalize_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return START_URL.to_string();
    }
    let has_scheme = trimmed.contains("://")
        || trimmed.starts_with("about:")
        || trimmed.starts_with("data:")
        || trimmed.starts_with("file:");
    if has_scheme {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

struct BrowserContent {
    state: BrowserState,
}

impl BrowserContent {
    fn new() -> Self {
        Self {
            state: BrowserState::new(),
        }
    }
}

struct BrowserDelegate {
    status: gtk::Label,
}

impl WebViewDelegate for BrowserDelegate {
    fn url_changed(&mut self, url: Option<&str>) {
        self.status.set_text(url.unwrap_or(""));
    }

    fn load_failed(&mut self, _url: Option<&str>, error: &str) {
        self.status.set_text(&format!(
            "{}: {error}",
            lang::t_or("browser.load_failed", "The page could not be loaded")
        ));
    }
}

impl Widget for BrowserContent {
    fn id(&self) -> WidgetId {
        0
    }

    fn to_gtk(&self) -> gtk::Widget {
        let state = &self.state;
        let web_view = state.web_view.clone();

        let web = WebView::new(
            WebKitConfiguration::new()
                .start_url(START_URL)
                .settings(
                    WebSettings::builder()
                        .user_agent("TontooOS, AppleWebKit")
                        .developer_extras(true)
                        .javascript_enabled(true)
                        .build(),
                ),
        )
        .expect("failed to create web view");
        web.set_delegate(Box::new(BrowserDelegate {
            status: state.status.clone(),
        }));
        *web_view.borrow_mut() = Some(web);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("webkit-toolbar");
        apply_css(
            &toolbar,
            ".webkit-toolbar { background-color: #1d1d1d; padding: 6px; }
             .webkit-toolbar button {
                 background-color: #2a2a2c;
                 color: #ececec;
                 border: 1px solid rgba(255, 255, 255, 0.12);
                 border-radius: 6px;
                 padding: 4px 12px;
                 font-family: 'SF Pro Display';
             }
             .webkit-toolbar button:hover { background-color: #3a3a3c; }
             .webkit-toolbar entry {
                 background-color: #2a2a2c;
                 color: #ececec;
                 caret-color: #ececec;
                 border: 1px solid rgba(255, 255, 255, 0.12);
                 border-radius: 6px;
             }",
        );

        let back = gtk::Button::with_label(&lang::t_or("webkit.back", "Back"));
        let w = web_view.clone();
        back.connect_clicked(move |_| {
            if let Some(v) = w.borrow().as_ref() {
                v.go_back();
            }
        });
        toolbar.append(&back);

        let forward = gtk::Button::with_label(&lang::t_or("webkit.forward", "Forward"));
        let w = web_view.clone();
        forward.connect_clicked(move |_| {
            if let Some(v) = w.borrow().as_ref() {
                v.go_forward();
            }
        });
        toolbar.append(&forward);

        let reload = gtk::Button::with_label(&lang::t_or("webkit.reload", "Reload"));
        let w = web_view.clone();
        reload.connect_clicked(move |_| {
            if let Some(v) = w.borrow().as_ref() {
                v.reload();
            }
        });
        toolbar.append(&reload);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(&lang::t_or("browser.address", "Address")));
        entry.set_text(START_URL);
        let w = web_view.clone();
        let status = state.status.clone();
        entry.connect_activate(move |entry| {
            navigate(&w, &status, &entry.text());
        });
        entry.set_hexpand(true);
        toolbar.append(&entry);

        let open = gtk::Button::with_label(&lang::t_or("browser.open", "Open"));
        let entry = entry.clone();
        let w = web_view.clone();
        let status = state.status.clone();
        open.connect_clicked(move |_| {
            navigate(&w, &status, &entry.text());
        });
        toolbar.append(&open);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&toolbar);

        let web_widget = web_view
            .borrow()
            .as_ref()
            .map(|v| v.widget())
            .expect("web view was created above");
        web_widget.set_vexpand(true);
        root.append(&web_widget);

        // F12 toggles the WebKit web inspector (Performance / CPU profile).
        let key_controller = gtk::EventControllerKey::new();
        let w = web_view.clone();
        key_controller.connect_key_pressed(move |_ctrl, key, _code, _state| {
            if key == gtk::gdk::keys::Key::F12 {
                if let Some(v) = w.borrow().as_ref() {
                    v.toggle_inspector();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        root.add_controller(key_controller);

        // Status bar: URL / errors on the left, web process stats on the right.
        let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        statusbar.set_margin_top(4);
        statusbar.set_margin_bottom(4);
        state.status.set_halign(gtk::Align::Start);
        state.status.set_hexpand(true);
        statusbar.append(&state.status);

        let perf_label = gtk::Label::new(None);
        perf_label.add_css_class("webkit-perf");
        apply_css(
            &perf_label,
            ".webkit-perf { color: #ececec; font-family: 'SF Pro Display'; font-size: 11px; }",
        );
        let perf = perf_label.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            let (cpu, mem) = webkit_process_stats();
            perf.set_text(&format!(
                "CPU {cpu:5.1}%  MEM {:6.1} MB",
                mem as f64 / 1_048_576.0
            ));
            glib::ControlFlow::Continue
        });
        statusbar.append(&perf_label);

        root.append(&statusbar);

        root.upcast()
    }
}

/// Aggregate CPU% and resident memory of every WebKitWebProcess.
fn webkit_process_stats() -> (f64, u64) {
    let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;

    fn sample() -> Vec<(u32, u64, u64, u64)> {
        let mut out = Vec::new();
        if let Ok(dir) = std::fs::read_dir("/proc") {
            for entry in dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let pid: u32 = match name.parse() {
                    Ok(pid) => pid,
                    Err(_) => continue,
                };
                if !is_webkit_process(pid) {
                    continue;
                }
                if let Some((utime, stime, rss)) = read_stat(pid) {
                    out.push((pid, utime, stime, rss));
                }
            }
        }
        out
    }

    let first = sample();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let second = sample();

    let mut cpu = 0.0f64;
    let mut rss = 0u64;
    for (pid, u1, s1, r1) in &first {
        if let Some((_, u2, s2, r2)) = second.iter().find(|(p, _, _, _)| p == pid) {
            let ticks = (u2 + s2).saturating_sub(u1 + s1);
            cpu += ticks as f64 / (0.4 * clk) * 100.0;
            rss += r2;
        }
    }
    (cpu, rss.saturating_mul(page))
}

fn is_webkit_process(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|c| c.trim() == "WebKitWebProcess")
        .unwrap_or(false)
        || std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .map(|c| c.contains("WebKitWebProcess"))
            .unwrap_or(false)
}

/// Fields 14/15 (utime/stime) and 24 (rss pages) from /proc/<pid>/stat.
fn read_stat(pid: u32) -> Option<(u64, u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rfind(')')? + 2;
    let rest: Vec<&str> = stat[after..].split_whitespace().collect();
    if rest.len() < 22 {
        return None;
    }
    let utime: u64 = rest[11].parse().ok()?;
    let stime: u64 = rest[12].parse().ok()?;
    let rss: u64 = rest[21].parse().ok()?;
    Some((utime, stime, rss))
}

struct BrowserApp;

impl AppDelegate for BrowserApp {
    fn view(&self) -> Box<dyn Widget> {
        Box::new(BrowserContent::new())
    }
}

fn main() {
    let mut app = App::new(lang::t_or("app.name", "Browser"), 1000, 680);
    app.set_color_scheme(ColorScheme::Dark);
    app.set_delegate(BrowserApp);
    app.run();
}