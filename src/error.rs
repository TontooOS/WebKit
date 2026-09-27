//! Error type shared by the whole framework.

use std::fmt;

/// Errors returned by the TontooWebKit framework.
#[derive(Debug, Clone)]
pub enum WebKitError {
    /// The URL is not a valid http(s), file, data or about URL.
    InvalidUrl(String),
    /// The web process failed to load the requested page.
    NavigationFailed(String),
    /// JavaScript evaluation failed.
    Javascript(String),
    /// An engine-level error.
    Engine(String),
    /// The web view has no usable settings object.
    NoSettings,
}

impl fmt::Display for WebKitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebKitError::InvalidUrl(u) => write!(f, "invalid URL: {u}"),
            WebKitError::NavigationFailed(e) => write!(f, "navigation failed: {e}"),
            WebKitError::Javascript(e) => write!(f, "javascript error: {e}"),
            WebKitError::Engine(e) => write!(f, "webkit engine error: {e}"),
            WebKitError::NoSettings => write!(f, "web view has no settings"),
        }
    }
}

impl std::error::Error for WebKitError {}

impl From<foundation::error::FoundationError> for WebKitError {
    fn from(e: foundation::error::FoundationError) -> Self {
        WebKitError::Engine(format!("json: {e}"))
    }
}