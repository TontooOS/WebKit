//! Download handling, the equivalent of `WKDownloadDelegate` in Apple
//! WebKit.
//!
//! Every download started by a [`crate::WebView`] (clicking a download
//! link, or an explicit `download_uri` call) is reported to the view's
//! [`DownloadDelegate`]. The delegate decides where the file is saved;
//! returning `None` from
//! [`DownloadDelegate::decide_destination`] cancels the download.

use webkit6 as wk;

/// A single in-progress download.
pub struct WebDownload {
    inner: wk::Download,
}

impl WebDownload {
    pub(crate) fn from_engine(inner: wk::Download) -> Self {
        Self { inner }
    }

    /// The URI being downloaded.
    pub fn uri(&self) -> Option<String> {
        self.inner
            .response()
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
    }

    /// The destination path chosen by the delegate, if already decided.
    pub fn destination(&self) -> Option<String> {
        self.inner.destination().map(|d| d.to_string())
    }

    /// Estimated progress between 0.0 and 1.0.
    pub fn estimated_progress(&self) -> f64 {
        self.inner.estimated_progress()
    }

    /// Bytes received so far.
    pub fn received_bytes(&self) -> u64 {
        self.inner.received_data_length()
    }

    /// Cancel the download.
    pub fn cancel(&self) {
        self.inner.cancel();
    }
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
