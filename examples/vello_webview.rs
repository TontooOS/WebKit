//! TontooUI (Vello) web view example.
//!
//! Shows the backend-neutral [`webkit::WebView`] inside a TontooUI window:
//! URL loading, texture-blit frames, scroll and mouse forwarding. Until the
//! WPE helper process lands, frames come from the mock engine.
//!
//! Run with: `cargo run --example vello_webview`

use tontooui::elements::layout::{Align, VStack, View};
use tontooui::renderer::images::ImageLoader;
use tontooui::renderer::text::FontSystem;
use tontooui::renderer::window::{App, Viewport};
use vello::Scene;
use webkit::{WebKitConfiguration, WebViewContent};

struct BrowserApp {
    root: VStack,
}

impl BrowserApp {
    fn new() -> Self {
        let web = WebViewContent::new(
            WebKitConfiguration::new().start_url("https://example.com"),
        )
        .expect("failed to create web view");
        Self {
            root: VStack::new().spacing(0.0).align(Align::Leading).child(web),
        }
    }
}

impl App for BrowserApp {
    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
        viewport: Viewport,
        _time_secs: f64,
    ) {
        let (w, h) = self.root.measure(fonts);
        self.root.place(fonts, viewport.x, viewport.y, w.max(viewport.width), h.max(viewport.height));
        self.root.draw(scene, fonts, images);
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        self.root.mouse_down(x, y);
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        self.root.mouse_up(x, y);
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        if let Some(web) = self.root.child_mut::<WebViewContent>(0) {
            web.mouse_wheel(dx, dy);
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(web) = self.root.child_mut::<WebViewContent>(0) {
            web.text(text);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tontooui::renderer::window::run("TontooWebKit", 900, 640, BrowserApp::new())
}
