//! TontooOS browser demo app built with UIKit + TontooWebKit.
//!
//! Shows how to embed a [`webkit::WebView`] in a UIKit app. The toolbar is
//! plain GTK4 so the buttons can capture the (non-`Send`) web view directly;
//! everything else follows the normal UIKit `App` / `AppDelegate` / `Widget`
//! flow.
//!
//! Run with: `cargo run --example browser`

use gtk::prelude::*;
use webkit6::prelude::*;
use uikit::app::{App, AppDelegate, ColorScheme};
use uikit::widget::{apply_css, Widget, WidgetId};
use webkit::{lang, WebKitConfiguration, WebSettings, WebView};

const START_URL: &str = "https://example.com";

#[derive(Clone)]
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

fn navigate(web_view: &std::rc::Rc<std::cell::RefCell<Option<WebView>>>, status: &gtk::Label, input: &str) {
    let url = normalize_url(input);
    match web_view.borrow().as_ref().map(|web| web.load_url(&url)) {
        Some(Ok(())) => {}
        Some(Err(e)) => {
            status.set_text(&format!("{}: {e}", lang::t_or("webkit.error", "Error")))
        }
        None => {}
    }
}

/// A single WebKit process as shown in the performance view.
struct ProcessStat {
    name: String,
    pid: u32,
    cpu: f64,
    mem: u64,
}

/// Aggregate CPU% and resident memory of every WebKit process, one entry per
/// process so the performance view can show the heaviest consumers.
fn webkit_processes() -> Vec<ProcessStat> {
    let clk = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;

    fn is_webkit(name: &str) -> bool {
        matches!(
            name,
            "WebKitWebProcess" | "WebKitNetworkProcess" | "WebKitGPUProcess"
        )
    }

    fn comm(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().to_string())
            .unwrap_or_default()
    }

    /// Fields utime/stime (14/15) and rss pages (24) from /proc/<pid>/stat.
    fn stat(pid: u32) -> Option<(u64, u64, u64)> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = s.rfind(')')? + 2;
        let rest: Vec<&str> = s[after..].split_whitespace().collect();
        if rest.len() < 22 {
            return None;
        }
        let utime: u64 = rest[11].parse().ok()?;
        let stime: u64 = rest[12].parse().ok()?;
        let rss: u64 = rest[21].parse().ok()?;
        Some((utime, stime, rss))
    }

    fn sample() -> Vec<(u32, String, u64, u64)> {
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
                let proc_name = comm(pid);
                if !is_webkit(&proc_name) {
                    continue;
                }
                if let Some((utime, stime, _rss)) = stat(pid) {
                    out.push((pid, proc_name, utime, stime));
                }
            }
        }
        out
    }

    let first = sample();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let second = sample();

    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let mut stats = Vec::new();
    for (pid, proc_name, u1, s1) in &first {
        if let Some((_, _, u2, s2)) = second.iter().find(|(p, _, _, _)| p == pid) {
            let ticks = (u2 + s2).saturating_sub(u1 + s1);
            let cpu = ticks as f64 / (0.4 * clk) * 100.0;
            // Re-read rss from the second sample.
            let mem = stat(*pid).map(|(_, _, r)| r).unwrap_or(0) * page;
            stats.push(ProcessStat {
                name: proc_name.clone(),
                pid: *pid,
                cpu,
                mem,
            });
        }
    }
    // Heaviest consumer first.
    stats.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    stats
}

/// Total CPU% + memory summary for the status bar.
fn webkit_process_summary() -> (usize, f64, u64) {
    let procs = webkit_processes();
    let count = procs.len();
    let cpu: f64 = procs.iter().map(|p| p.cpu).sum();
    let mem: u64 = procs.iter().map(|p| p.mem).sum();
    (count, cpu, mem)
}

/// (Re)fill the performance window's list with the current process stats.
fn refresh_perf_window(list: &gtk::ListBox) {
    list.remove_all();
    for proc in webkit_processes() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&format!(
            "{:<22} pid {:<7} CPU {:>5.1}%  {:>7.1} MB",
            proc.name, proc.pid, proc.cpu, proc.mem as f64 / 1_048_576.0
        )));
        label.add_css_class("dim-label");
        row.append(&label);
        list.append(&gtk::ListBoxRow::builder().child(&row).build());
    }
}

fn show_performance_window() {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_margin_top(8);
    list.set_margin_bottom(8);
    list.set_margin_start(8);
    list.set_margin_end(8);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);

    let window = gtk::Window::new();
    window.set_title(Some("TontooWebKit - Performance"));
    window.set_default_size(440, 400);
    window.set_child(Some(&scroll));

    refresh_perf_window(&list);

    let list_c = list.clone();
    glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        refresh_perf_window(&list_c);
        glib::ControlFlow::Continue
    });

    window.present();
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

impl webkit::WebViewDelegate for BrowserDelegate {
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
                        .javascript_enabled(true)
                        .build(),
                ),
        )
        .expect("failed to create web view");
        web.set_delegate(Box::new(BrowserDelegate {
            status: state.status.clone(),
        }));

        // Show the hovered link in the status bar; fall back to the
        // current page URL when the pointer leaves a link.
        let status = state.status.clone();
        web.inner().connect_mouse_target_changed(move |wv, hit, _mods| {
            if let Some(link) = hit.link_uri() {
                status.set_text(link.as_str());
            } else {
                let url = wv.uri().map(|u| u.to_string()).unwrap_or_default();
                status.set_text(url.as_str());
            }
        });
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
             }
             .webkit-status { color: #a0a0a0; font-family: 'SF Pro Display'; font-size: 11px; }",
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

        let perf_btn = gtk::Button::with_label("Performance");
        perf_btn.connect_clicked(move |_| show_performance_window());
        toolbar.append(&perf_btn);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(&lang::t_or("browser.address", "Address")));
        entry.set_text(START_URL);
        let status = state.status.clone();
        let w = web_view.clone();
        entry.connect_activate(move |entry| {
            navigate(&w, &status, &entry.text());
        });
        entry.set_hexpand(true);
        toolbar.append(&entry);

        let open = gtk::Button::with_label(&lang::t_or("browser.open", "Open"));
        let entry = entry.clone();
        let status = state.status.clone();
        let w = web_view.clone();
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

        // Status bar with a summary of WebKit process CPU / memory.
        let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        statusbar.set_margin_top(4);
        statusbar.set_margin_bottom(4);
        state.status.set_halign(gtk::Align::Start);
        state.status.set_hexpand(true);
        // Cap the natural width so long URLs ellipsize instead of
        // resizing the window.
        state.status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        state.status.set_max_width_chars(64);
        state.status.add_css_class("webkit-status");
        statusbar.append(&state.status);

        let perf_label = gtk::Label::new(None);
        perf_label.add_css_class("webkit-status");
        let perf = perf_label.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            let (count, cpu, mem) = webkit_process_summary();
            perf.set_text(&format!(
                "WebKit: {count} process(es)  CPU {cpu:5.1}%  MEM {:>7.1} MB",
                mem as f64 / 1_048_576.0
            ));
            glib::ControlFlow::Continue
        });
        statusbar.append(&perf_label);

        root.append(&statusbar);

        // F12 opens the performance window.
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_ctrl, key, _code, _state| {
            if key == gtk::gdk::Key::F12 {
                show_performance_window();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        root.add_controller(key_controller);

        root.upcast()
    }
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