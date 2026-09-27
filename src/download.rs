//! Download handling, the equivalent of `WKDownloadDelegate` in Apple
//! WebKit.
//!
//! [`DownloadDelegate`] is backend-neutral. [`WebDownload`] carries plain
//! data for the out-of-process engine.

/// A single in-progress download (backend-neutral descriptor).
///
/// Used by the out-of-process engine.
#[derive(Debug, Clone)]
pub struct WebDownload {
    /// The URI being downloaded.
    pub uri: Option<String>,
    /// The destination path chosen by the delegate, if already decided.
    pub destination: Option<String>,
    /// Estimated progress between 0.0 and 1.0.
    pub progress: f64,
    /// Bytes received so far.
    pub received_bytes: u64,
}

impl WebDownload {
    /// The URI being downloaded.
    pub fn uri(&self) -> Option<String> {
        self.uri.clone()
    }

    /// The destination path chosen by the delegate, if already decided.
    pub fn destination(&self) -> Option<String> {
        self.destination.clone()
    }

    /// Estimated progress between 0.0 and 1.0.
    pub fn estimated_progress(&self) -> f64 {
        self.progress
    }

    /// Bytes received so far.
    pub fn received_bytes(&self) -> u64 {
        self.received_bytes
    }

    /// Cancel the download.
    pub fn cancel(&self) {}
}

/// Delegate for downloads started by a [`crate::WebView`].
///
/// All methods have default implementations. The default
/// `decide_destination` returns `None`, which cancels every download --
/// override it to actually save files.
pub trait DownloadDelegate {
    /// Decide where the download is saved. Return the full destination
    /// file path, or `None` to cancel the download.
    fn decide_destination(
        &mut self,
        _download: &WebDownload,
        _suggested_filename: &str,
    ) -> Option<String> {
        None
    }

    /// The download failed.
    fn download_failed(&mut self, _download: &WebDownload, _error: &str) {}

    /// The download finished successfully.
    fn download_finished(&mut self, _download: &WebDownload) {}

    /// Download progress changed.
    fn download_progress(&mut self, _download: &WebDownload) {}
}

/// Default delegate: cancels every download by not choosing a
/// destination.
#[derive(Debug, Default)]
pub struct DefaultDownloadDelegate;

impl DownloadDelegate for DefaultDownloadDelegate {}
