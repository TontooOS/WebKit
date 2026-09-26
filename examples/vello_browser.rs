//! Full TontooWebKit browser demo on TontooUI (Vello).
//!
//! Toolbar with back/forward/reload/go, an address field (Enter commits),
//! a progress bar, the page texture and a status line with title and URL.
//! Spawns `tontoo-webengine` when available, otherwise uses the test
//! engine (animated placeholder frames).
//!
//! Run with: `cargo run --example vello_browser`
//!
//! Try: type `example.com` + Enter, click Back/Reload, watch progress.

use std::cell::RefCell;
use std::rc::Rc;

use tontooui::elements::{
    BasicText, BasicTextField, Button, LinearProgress, Titlebar, TrafficAction, View,
};
use tontooui::renderer::FontSystem;
use tontooui::renderer::ImageLoader;
use tontooui::renderer::window::{App, CursorKind, Key, Viewport, WindowCommand, run};
use tontooui::theme::{ThemeMode, ThemeWatcher};
use vello::Scene;
use vello::peniko::Color;
use webkit::WebViewContent;

#[derive(Clone, Copy)]
enum NavCmd {
    Back,
    Forward,
    Reload,
    Go,
}

fn normalize_url(text: &str) -> String {
    let text = text.trim().to_string();
    if text.is_empty() {
        return "https://example.com".into();
    }
    let lower = text.to_ascii_lowercase();
    for prefix in ["http://", "https://", "file://", "data:", "about:"] {
        if lower.starts_with(prefix) {
            return text;
        }
    }
    format!("https://{text}")
}

struct BrowserDemo {
    bar: Titlebar,
    back: Button,
    forward: Button,
    reload: Button,
    go: Button,
    url: BasicTextField,
    progress: LinearProgress,
    web: WebViewContent,
    status: BasicText,
    nav: Rc<RefCell<Option<NavCmd>>>,
    pending_keys: Vec<Key>,
    watcher: ThemeWatcher,
    focused: bool,
    bg: Color,
    command: Option<WindowCommand>,
}

impl BrowserDemo {
    fn new() -> Self {
        let nav: Rc<RefCell<Option<NavCmd>>> = Rc::new(RefCell::new(None));
        let press = |nav: &Rc<RefCell<Option<NavCmd>>>, cmd: NavCmd| {
            let nav = nav.clone();
            move || *nav.borrow_mut() = Some(cmd)
        };
        let web = WebViewContent::with_spawned_fallback();
        let mut url = BasicTextField::new("Search or enter address");
        url.set_text("https://example.com");
        Self {
            bar: Titlebar::new("Tontoo Browser"),
            back: Button::new("<").on_press(press(&nav, NavCmd::Back)),
            forward: Button::new(">").on_press(press(&nav, NavCmd::Forward)),
            reload: Button::new("Reload").on_press(press(&nav, NavCmd::Reload)),
            go: Button::new("Go").on_press(press(&nav, NavCmd::Go)),
            url,
            progress: LinearProgress::new(),
            web,
            status: BasicText::new("Ready."),
            nav,
            pending_keys: Vec::new(),
            watcher: ThemeWatcher::new(),
            focused: true,
            bg: tontooui::renderer::window::BACKGROUND,
            command: None,
        }
    }

    fn navigate(&mut self, cmd: NavCmd) {
        let view = self.web.web_view();
        match cmd {
            NavCmd::Back => view.go_back(),
            NavCmd::Forward => view.go_forward(),
            NavCmd::Reload => view.reload(),
            NavCmd::Go => {
                let target = normalize_url(self.url.text_value());
                self.url.set_text(target.clone());
                if let Err(e) = view.load_url(&target) {
                    self.status
                        .set_text(format!("Invalid URL: {e}"));
                }
            }
        }
    }
}

impl App for BrowserDemo {
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
        self.web.web_view().pump_events();
        let cmd = self.nav.borrow_mut().take();
        if let Some(cmd) = cmd {
            self.navigate(cmd);
        }
        for key in std::mem::take(&mut self.pending_keys) {
            if key == Key::Enter {
                if self.url.is_selected() {
                    self.navigate(NavCmd::Go);
                }
                continue;
            }
            if self.url.is_selected() {
                self.url.key(key);
            }
        }

        let view = self.web.web_view();
        self.progress.set_progress(view.estimated_progress());
        let status = if view.is_loading() {
            format!(
                "Loading… {}%  {}",
                (view.estimated_progress() * 100.0).round(),
                view.url().unwrap_or_default()
            )
        } else {
            format!(
                "{}  {}",
                view.title().unwrap_or_else(|| "Untitled".into()),
                view.url().unwrap_or_default()
            )
        };
        self.status.set_text(status);

        self.back.set_theme(palette.accent, dark);
        self.back.set_focused(focused);
        self.forward.set_theme(palette.accent, dark);
        self.forward.set_focused(focused);
        self.reload.set_theme(palette.accent, dark);
        self.reload.set_focused(focused);
        self.go.set_theme(palette.accent, dark);
        self.go.set_focused(focused);
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

        // Toolbar row under the titlebar.
        let cx = viewport.x + 12.0;
        let mut x = cx;
        let y = viewport.y + 31.0 + 8.0;
        let content_w = viewport.width - 24.0;
        for button in [&mut self.back, &mut self.forward, &mut self.reload] {
            let (bw, bh) = button.measure(fonts);
            button.place(fonts, x, y, bw, bh.max(30.0));
            button.draw(scene, fonts, images);
            x += bw + 8.0;
        }
        let (gw, gh) = self.go.measure(fonts);
        let (_, uh) = self.url.measure(fonts);
        let row_h = uh.max(30.0).max(gh);
        let field_w = (viewport.x + content_w - gw - 8.0 - x).max(120.0);
        self.url.place(fonts, x, y, field_w, row_h);
        self.url.draw(scene, fonts, images);
        self.go.place(fonts, x + field_w + 8.0, y, gw, row_h);
        self.go.draw(scene, fonts, images);

        // Progress hairline, then the page, then the status line.
        let py = y + row_h + 6.0;
        self.progress.place(fonts, cx, py, content_w, 6.0);
        self.progress.draw(scene, fonts, images);
        let (sw, sh) = self.status.measure(fonts);
        let web_y = py + 10.0;
        let web_h = (viewport.y + viewport.height - 4.0 - (sh + 6.0) - web_y).max(100.0);
        self.web.place(fonts, cx, web_y, content_w, web_h);
        self.web.draw(scene, fonts, images);
        self.status.place(fonts, cx, web_y + web_h + 6.0, sw.min(content_w), sh);
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
                self.back.mouse_down(x, y);
                self.forward.mouse_down(x, y);
                self.reload.mouse_down(x, y);
                self.go.mouse_down(x, y);
                self.url.mouse_down(x, y);
                self.web.mouse_down(x, y);
            }
        }
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.back.mouse_up(x, y);
        self.forward.mouse_up(x, y);
        self.reload.mouse_up(x, y);
        self.go.mouse_up(x, y);
        self.web.mouse_up(x, y);
    }

    fn mouse_move(&mut self, x: f64, y: f64) {
        self.bar.set_hover(x as f32, y as f32);
        self.back.set_hover(x as f32, y as f32);
        self.forward.set_hover(x as f32, y as f32);
        self.reload.set_hover(x as f32, y as f32);
        self.go.set_hover(x as f32, y as f32);
        self.url.set_hover(x as f32, y as f32);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        self.web.mouse_wheel(dx, dy);
    }

    fn text(&mut self, text: &str) {
        if self.url.is_selected() {
            self.url.type_text(text);
        } else {
            self.web.text(text);
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
    if let Err(err) = run("Tontoo Browser", 1000, 700, BrowserDemo::new()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
