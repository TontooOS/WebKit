//! TontooOS browser: a TontooUI toolbar window on top of headful Firefox.
//!
//! The toolbar is a normal TontooUI window (traffic lights, address field,
//! back/forward/reload/new tab, progress and a status line). Firefox runs
//! as its own Wayland window with `--kiosk`-style chrome hidden and is
//! driven over WebDriver BiDi by [`webkit::GeckoEngine`], so pages render at
//! full frame rate with WebExtensions enabled.
//!
//! Run with: `cargo run --example tontoo_browser`
//!
//! Try: drag the toolbar to the top edge, click the address field, type
//! `example.com` + Enter, then click into the page (the toolbar only takes
//! keyboard focus while it is focused, otherwise keys go to Firefox).
//!
//! Env:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `TONTOO_FIREFOX_BIN` | Firefox binary override |
//! | `TONTOO_FIREFOX_CHANNEL` | `esr` (default) or `release` for provisioning |
//! | `TONTOO_BROWSER_HOME` | Start URL (default `https://example.com`) |
//! | `TONTOO_BROWSER_EXTENSIONS` | Comma separated AMO add-on ids to install |
//! | `TONTOO_BROWSER_PRIVATE` | `1` starts an ephemeral private profile |
//! | `TONTOO_BROWSER_HEADLESS` | `1` runs Firefox without a window (CI) |

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use tontooui::elements::{
    BasicText, BasicTextField, Button, LinearProgress, Titlebar, TrafficAction, View,
};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, CursorKind, Key, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;
use webkit::{ExtensionPolicy, GeckoEngine, GeckoOptions, WebKitConfiguration, WebView};

/// Toolbar window size in logical px. The compositor places windows, so
/// the toolbar is draggable (drag the titlebar) to sit above the page.
const TOOLBAR_WIDTH: u32 = 1000;
const TOOLBAR_HEIGHT: u32 = 92;

fn env_flag(name: &str) -> bool {
    matches!(std::env::var(name).as_deref(), Ok("1") | Ok("true"))
}

fn start_url() -> String {
    std::env::var("TONTOO_BROWSER_HOME").unwrap_or_else(|_| "https://example.com".into())
}

fn download_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    std::path::PathBuf::from(base)
        .join("tontoo-webengine")
        .join("downloads")
}

/// Add-ons the managed profile installs at startup.
///
/// `TONTOO_BROWSER_EXTENSIONS` is a comma separated list of AMO add-on
/// ids; the default is the TontooOS curated set. Firefox downloads and
/// updates them itself through the profile policy.
fn managed_extensions() -> Vec<ExtensionPolicy> {
    let list = std::env::var("TONTOO_BROWSER_EXTENSIONS")
        .unwrap_or_else(|_| "uBlock0@raymondhill.net,jid1-MnnxcxisBPnSXQ@jetpack".into());
    list.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| ExtensionPolicy::AddonId(id.to_string()))
        .collect()
}

/// Turn typed text into a URL (`example.com` -> `https://example.com`).
fn normalize_url(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return start_url();
    }
    let lower = text.to_ascii_lowercase();
    for prefix in ["http://", "https://", "file://", "data:", "about:"] {
        if lower.starts_with(prefix) {
            return text.to_string();
        }
    }
    format!("https://{text}")
}

#[derive(Clone, Copy)]
enum NavCmd {
    Back,
    Forward,
    Reload,
    Go,
    NewTab,
}

struct Toolbar {
    bar: Titlebar,
    back: Button,
    forward: Button,
    reload: Button,
    tab: Button,
    url: BasicTextField,
    progress: LinearProgress,
    status: BasicText,
    view: WebView,
    nav: Rc<RefCell<Option<NavCmd>>>,
    pending_keys: Vec<Key>,
    watcher: ThemeWatcher,
    focused: bool,
    bg: Color,
    command: Option<WindowCommand>,
    /// Set when the engine could not be started at all.
    failed: Option<String>,
}

impl Toolbar {
    fn new() -> Self {
        let nav: Rc<RefCell<Option<NavCmd>>> = Rc::new(RefCell::new(None));
        let press = |nav: &Rc<RefCell<Option<NavCmd>>>, cmd: NavCmd| {
            let nav = nav.clone();
            move || *nav.borrow_mut() = Some(cmd)
        };
        let (view, failed) = match spawn_engine() {
            Ok(view) => (view, None),
            Err(e) => {
                eprintln!("tontoo_browser: {e}");
                (
                    WebView::new(WebKitConfiguration::new()).expect("mock view always builds"),
                    Some(e),
                )
            }
        };
        let mut url = BasicTextField::new("Search or enter address");
        url.set_text(view.url().unwrap_or_else(start_url));
        Self {
            bar: Titlebar::new("Tontoo Browser"),
            back: Button::new("Back").on_press(press(&nav, NavCmd::Back)),
            forward: Button::new("Forward").on_press(press(&nav, NavCmd::Forward)),
            reload: Button::new("Reload").on_press(press(&nav, NavCmd::Reload)),
            tab: Button::new("New Tab").on_press(press(&nav, NavCmd::NewTab)),
            url,
            progress: LinearProgress::new(),
            status: BasicText::new("Starting Firefox..."),
            view,
            nav,
            pending_keys: Vec::new(),
            watcher: ThemeWatcher::new(),
            focused: true,
            bg: tontooui::renderer::window::BACKGROUND,
            command: None,
            failed,
        }
    }

    fn navigate(&mut self, cmd: NavCmd) {
        match cmd {
            NavCmd::Back => self.view.go_back(),
            NavCmd::Forward => self.view.go_forward(),
            NavCmd::Reload => self.view.reload(),
            NavCmd::Go => {
                let target = normalize_url(self.url.text_value());
                self.url.set_text(target.clone());
                if let Err(e) = self.view.load_url(&target) {
                    self.status.set_text(format!("Invalid URL: {e}"));
                }
            }
            NavCmd::NewTab => {
                self.view.new_tab("window");
                self.status.set_text("New window requested");
            }
        }
    }

    /// Forward a key the address field did not want to Firefox.
    fn forward_key(&self, key: Key) {
        let name = match key {
            Key::Enter => "Enter",
            Key::Backspace => "Backspace",
            Key::Escape => "Escape",
            Key::Left => "ArrowLeft",
            Key::Right => "ArrowRight",
            Key::Up => "ArrowUp",
            Key::Down => "ArrowDown",
            _ => return,
        };
        self.view.press_key(name);
    }
}

/// Launch Firefox and wrap it in a web view.
fn spawn_engine() -> Result<WebView, String> {
    let downloads = download_dir();
    std::fs::create_dir_all(&downloads).map_err(|e| format!("download dir: {e}"))?;
    let options = GeckoOptions {
        // Chrome hidden: the TontooUI toolbar is the only browser chrome.
        kiosk: true,
        headless: env_flag("TONTOO_BROWSER_HEADLESS"),
        private: env_flag("TONTOO_BROWSER_PRIVATE"),
        download_dir: downloads,
        start_url: Some(start_url()),
        provision: true,
        extensions: managed_extensions(),
    };
    let engine = GeckoEngine::launch(options)?;
    let config = WebKitConfiguration::new().start_url(start_url());
    WebView::with_engine(config, Arc::new(engine))
        .map_err(|e| format!("web view: {}", e))
        .map_err(|e: String| e)
}

impl App for Toolbar {
    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
        viewport: Viewport,
        time_secs: f64,
    ) {
        self.watcher.poll(time_secs);
        self.watcher.set_focused(self.focused, time_secs);
        let palette = self.watcher.palette(time_secs);
        self.bg = palette.bg;
        let theme = self.watcher.theme();
        let dark = theme.mode == ThemeMode::Dark;
        let focused = self.focused;

        // Fresh engine state before reading title/progress below.
        self.view.pump_events();
        let queued = self.nav.borrow_mut().take();
        if let Some(cmd) = queued {
            self.navigate(cmd);
        }
        for key in std::mem::take(&mut self.pending_keys) {
            if self.url.is_selected() {
                if key == Key::Enter {
                    self.navigate(NavCmd::Go);
                    continue;
                }
                self.url.key(key);
            } else {
                self.forward_key(key);
            }
        }

        self.progress.set_progress(self.view.estimated_progress());
        let (progress, loading) = (self.view.estimated_progress(), self.view.is_loading());
        let url = self.view.url().unwrap_or_default();
        let title = self.view.title().unwrap_or_else(|| "Untitled".into());
        self.status.set_text(if let Some(failed) = &self.failed {
            format!("Engine unavailable: {failed}")
        } else if loading {
            format!("Loading {}%  {url}", (progress * 100.0).round())
        } else {
            format!("{title}  {url}")
        });

        for button in [
            &mut self.back,
            &mut self.forward,
            &mut self.reload,
            &mut self.tab,
        ] {
            button.set_theme(palette.accent, dark);
            button.set_focused(focused);
        }
        self.url.set_theme(palette.accent, dark);
        self.url.set_focused(focused);
        self.progress.set_theme(palette.accent, dark);
        self.progress.set_focused(focused);
        self.status.set_theme(theme.mode);
        self.status.set_focused(focused);
        self.bar.set_palette(
            palette.titlebar_bg,
            palette.titlebar_text,
            palette.divider,
        );
        self.bar.set_rect(viewport.x, viewport.y, viewport.width);
        self.bar.draw(scene, fonts);

        let x0 = viewport.x + 12.0;
        let mut x = x0;
        let y = viewport.y + 31.0 + 6.0;
        let row_h = 30.0f32;
        for button in [
            &mut self.back,
            &mut self.forward,
            &mut self.reload,
            &mut self.tab,
        ] {
            let (bw, bh) = button.measure(fonts);
            button.place(fonts, x, y, bw, bh.max(row_h));
            button.draw(scene, fonts, images);
            x += bw + 8.0;
        }
        let width = (viewport.x + viewport.width - 12.0 - x).max(120.0);
        let (_, uh) = self.url.measure(fonts);
        let field_h = uh.max(row_h);
        self.url.place(fonts, x, y, width, field_h);
        self.url.draw(scene, fonts, images);

        let py = y + field_h + 6.0;
        self.progress
            .place(fonts, x0, py, viewport.width - 24.0, 6.0);
        self.progress.draw(scene, fonts, images);
        let (sw, sh) = self.status.measure(fonts);
        self.status
            .place(fonts, x0, py + 10.0, sw.min(viewport.width - 24.0), sh);
        self.status.draw(scene, fonts, images);
    }

    fn background(&self) -> Color {
        self.bg
    }

    fn drag_region(&self) -> Option<(f32, f32, f32, f32)> {
        Some(self.bar.drag_rect())
    }

    fn poll_window_command(&mut self) -> Option<WindowCommand> {
        self.command.take()
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        match self.bar.press(x as f32, y as f32) {
            Some(TrafficAction::Close) => self.command = Some(WindowCommand::Close),
            Some(TrafficAction::Minimize) => self.command = Some(WindowCommand::Minimize),
            Some(TrafficAction::Maximize) => self.command = Some(WindowCommand::ToggleMaximize),
            None => {
                for button in [
                    &mut self.back,
                    &mut self.forward,
                    &mut self.reload,
                    &mut self.tab,
                ] {
                    button.mouse_down(x, y);
                }
                self.url.mouse_down(x, y);
            }
        }
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        for button in [
            &mut self.back,
            &mut self.forward,
            &mut self.reload,
            &mut self.tab,
        ] {
            button.mouse_up(x, y);
        }
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        self.bar.set_hover(x as f32, y as f32);
        for button in [
            &mut self.back,
            &mut self.forward,
            &mut self.reload,
            &mut self.tab,
        ] {
            button.set_hover(x as f32, y as f32);
        }
        self.url.set_hover(x as f32, y as f32);
    }

    fn text(&mut self, text: &str) {
        if self.url.is_selected() {
            self.url.type_text(text);
        } else {
            self.view.input_text(text);
        }
    }

    fn key(&mut self, key: Key) {
        self.pending_keys.push(key);
    }

    fn cursor(&self, _x: f64, _y: f64) -> CursorKind {
        if self.url.wants_text_cursor() {
            CursorKind::Text
        } else {
            CursorKind::Default
        }
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.bar.set_focused(focused);
    }
}

fn main() {
    let toolbar = Toolbar::new();
    if let Err(err) = run(
        "Tontoo Browser",
        TOOLBAR_WIDTH,
        TOOLBAR_HEIGHT,
        toolbar,
    ) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
