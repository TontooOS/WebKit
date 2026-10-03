//! TontooUI embedding for the backend-neutral [`crate::WebView`].
//!
//! [`WebViewContent`] implements `tontooui::elements::layout::View`, so a
//! web view joins any TontooUI view tree like a label or a button. Each
//! frame blits the latest [`crate::engine::SharedFrame`] as a Vello texture
//! (rounded corners + LiquidGlass keep working); input is forwarded to the
//! engine as commands.

use std::any::Any;

use tontooui::elements::layout::View;
use tontooui::renderer::images::ImageLoader;
use tontooui::renderer::text::FontSystem;
use tontooui::renderer::window::Key;
use vello::Scene;
use vello::kurbo::{Affine, RoundedRect};
use vello::peniko::Fill;

use crate::config::WebKitConfiguration;
use crate::error::WebKitError;
use crate::view::WebView;

/// Corner radius of the web frame in logical px.
pub const WEBVIEW_RADIUS: f32 = 12.0;

/// A TontooUI-compatible wrapper around a [`WebView`].
///
/// ```rust,no_run
/// use tontooui::elements::layout::{View, VStack};
/// use webkit::{WebKitConfiguration, WebViewContent};
///
/// let web = WebViewContent::new(
///     WebKitConfiguration::new().start_url("https://example.com"),
/// ).expect("failed to create web view");
/// let stack = VStack::new().child(web);
/// ```
pub struct WebViewContent {
    inner: WebView,
    x: f32,
    y: f32,
    placed_w: f32,
    placed_h: f32,
    scale: f32,
}

impl WebViewContent {
    /// Create the TontooUI content wrapper from a configuration.
    pub fn new(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        Ok(Self {
            inner: WebView::new(config)?,
            x: 0.0,
            y: 0.0,
            placed_w: 0.0,
            placed_h: 0.0,
            scale: 1.0,
        })
    }

    /// Create the wrapper on an explicit engine, e.g. the in-process
    /// [`crate::wpe::WpeEngine`].
    pub fn with_engine(
        config: WebKitConfiguration,
        engine: std::sync::Arc<dyn crate::engine::WebEngine>,
    ) -> Result<Self, WebKitError> {
        Ok(Self {
            inner: WebView::with_engine(config, engine)?,
            x: 0.0,
            y: 0.0,
            placed_w: 0.0,
            placed_h: 0.0,
            scale: 1.0,
        })
    }

    /// Create the wrapper on the in-process WPE WebKit engine.
    ///
    /// The engine renders offscreen, so the page is drawn inside this window
    /// like any other TontooUI view. Returns the engine error when the WPE
    /// libraries are missing.
    #[cfg(feature = "wpe")]
    pub fn with_wpe(config: WebKitConfiguration) -> Result<Self, WebKitError> {
        let engine = crate::wpe::WpeEngine::launch(crate::wpe::WpeOptions::default())
            .map_err(WebKitError::Engine)?;
        Self::with_engine(config, std::sync::Arc::new(engine))
    }

    /// Create the wrapper on the spawned helper when available,
    /// otherwise on the test engine. Never fails.
    pub fn with_spawned_fallback() -> Self {
        let inner = WebView::with_spawned_engine(WebKitConfiguration::new())
            .expect("mock engine always builds");
        Self {
            inner,
            x: 0.0,
            y: 0.0,
            placed_w: 0.0,
            placed_h: 0.0,
            scale: 1.0,
        }
    }

    /// The wrapped [`WebView`].
    pub fn web_view(&self) -> &WebView {
        &self.inner
    }

    /// Placed rect (x, y, width, height) in logical px.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.placed_w, self.placed_h)
    }

    fn input_coords(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.placed_w <= 0.0 || self.placed_h <= 0.0 {
            return None;
        }
        let lx = x as f32 - self.x;
        let ly = y as f32 - self.y;
        if lx < 0.0 || ly < 0.0 || lx > self.placed_w || ly > self.placed_h {
            return None;
        }
        Some((lx as f64, ly as f64))
    }
}

impl View for WebViewContent {
    fn measure(&mut self, _fonts: &mut FontSystem) -> (f32, f32) {
        (800.0, 600.0)
    }

    fn place(&mut self, fonts: &mut FontSystem, x: f32, y: f32, w: f32, h: f32) {
        self.x = x;
        self.y = y;
        self.placed_w = w.max(0.0);
        self.placed_h = h.max(0.0);
        self.scale = fonts.scale.max(0.5);
        if self.placed_w > 0.0 && self.placed_h > 0.0 {
            self.inner.resize(
                (self.placed_w * self.scale).ceil().max(1.0) as u32,
                (self.placed_h * self.scale).ceil().max(1.0) as u32,
                self.scale,
            );
        }
    }

    fn draw(
        &mut self,
        scene: &mut Scene,
        fonts: &mut FontSystem,
        images: &mut ImageLoader<'_>,
    ) {
        if self.placed_w <= 0.0 || self.placed_h <= 0.0 {
            return;
        }
        let Some(frame) = self.inner.poll_frame() else {
            return;
        };
        let Some((image, iw, ih)) =
            images.upload_frame(frame.seq, &frame.rgba, frame.width, frame.height)
        else {
            return;
        };
        let scale = fonts.scale as f64;
        let px = |v: f32| v as f64 * scale;
        let radius = WEBVIEW_RADIUS
            .min(self.placed_w / 2.0)
            .min(self.placed_h / 2.0)
            .max(0.0);
        let clip = RoundedRect::new(
            px(self.x),
            px(self.y),
            px(self.x + self.placed_w),
            px(self.y + self.placed_h),
            px(radius),
        );
        // Stretch the engine frame over the placed rect (the engine was
        // sized from this rect in `place`, so this is 1:1 in practice).
        let sx = self.placed_w as f64 * scale / iw.max(1) as f64;
        let sy = self.placed_h as f64 * scale / ih.max(1) as f64;
        let transform =
            Affine::translate((px(self.x), px(self.y))) * Affine::scale_non_uniform(sx, sy);
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &clip);
        scene.draw_image(&image, transform);
        scene.pop_layer();
    }

    fn mouse_down(&mut self, x: f64, y: f64) {
        if let Some((lx, ly)) = self.input_coords(x, y) {
            self.inner.mouse_down(lx, ly);
        }
    }

    fn mouse_up(&mut self, x: f64, y: f64) {
        if let Some((lx, ly)) = self.input_coords(x, y) {
            self.inner.mouse_up(lx, ly);
        }
    }

    fn mouse_wheel(&mut self, dx: f64, dy: f64) {
        self.inner.scroll(dx, dy);
    }

    fn text(&mut self, text: &str) {
        self.inner.input_text(text);
    }

    fn key(&mut self, key: Key) -> bool {
        let name = match key {
            Key::Enter => "Enter",
            Key::Backspace => "Backspace",
            Key::Escape => "Escape",
            Key::Left => "ArrowLeft",
            Key::Right => "ArrowRight",
            Key::Up => "ArrowUp",
            Key::Down => "ArrowDown",
            _ => return false,
        };
        self.inner.press_key(name);
        true
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
