//! User scripts and the JavaScript-to-Rust message bridge.
//!
//! A [`WebScript`] is injected into every page (or only the top frame) at
//! document start or document end. A [`ScriptMessageHandler`] registers a
//! named channel: JavaScript calls
//! `window.webkit.messageHandlers.<name>.postMessage(payload)` and the
//! payload arrives as a Foundation `JsonValue` in Rust.

/// When a user script is injected into the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptInjectionTime {
    /// Injected before the document is parsed.
    AtDocumentStart,
    /// Injected after the document is parsed.
    #[default]
    AtDocumentEnd,
}

/// Which frames receive the user script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptFrameInjection {
    /// Inject into the top frame only.
    #[default]
    TopFrame,
    /// Inject into every frame.
    AllFrames,
}

/// A user script injected into loaded pages.
#[derive(Debug, Clone, Default)]
pub struct WebScript {
    /// The JavaScript source code.
    pub source: String,
    /// When the script runs.
    pub injection_time: ScriptInjectionTime,
    /// Which frames receive the script.
    pub frames: ScriptFrameInjection,
    /// Only inject into URLs on this allow list (empty = all URLs).
    pub allow_list: Vec<String>,
    /// Never inject into URLs on this block list.
    pub block_list: Vec<String>,
}

impl WebScript {
    /// Create a script that runs at document end in the top frame.
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            injection_time: ScriptInjectionTime::AtDocumentEnd,
            frames: ScriptFrameInjection::TopFrame,
            allow_list: Vec::new(),
            block_list: Vec::new(),
        }
    }

    pub fn at_document_start(mut self) -> Self {
        self.injection_time = ScriptInjectionTime::AtDocumentStart;
        self
    }

    pub fn at_document_end(mut self) -> Self {
        self.injection_time = ScriptInjectionTime::AtDocumentEnd;
        self
    }

    pub fn in_all_frames(mut self) -> Self {
        self.frames = ScriptFrameInjection::AllFrames;
        self
    }

    pub fn allow_urls(mut self, urls: Vec<String>) -> Self {
        self.allow_list = urls;
        self
    }

    pub fn block_urls(mut self, urls: Vec<String>) -> Self {
        self.block_list = urls;
        self
    }
}

/// A named JavaScript-to-Rust message channel.
///
/// The [`WebView`](crate::WebView) registers the handler so that JavaScript
/// running in the page can call
/// `window.webkit.messageHandlers.<name>.postMessage(payload)`. The payload
/// is delivered to `body` as JSON.
///
/// ```rust,no_run
/// use webkit::{ScriptMessageHandler, WebKitConfiguration};
///
/// let config = WebKitConfiguration::new().add_message_handler(
///     ScriptMessageHandler::new("ready", |body| {
///         println!("page says: {body}");
///     }),
/// );
/// ```
pub struct ScriptMessageHandler {
    /// Name of the channel (must match the JavaScript handler name).
    pub name: String,
    /// Called with the JSON payload whenever the page posts a message.
    pub body: Box<dyn Fn(foundation::serialization::JsonValue) + 'static>,
}

impl ScriptMessageHandler {
    pub fn new(
        name: impl Into<String>,
        body: impl Fn(foundation::serialization::JsonValue) + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            body: Box::new(body),
        }
    }
}