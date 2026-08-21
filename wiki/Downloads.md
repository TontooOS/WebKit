# Downloads

Download support for TontooWebKit. Every download started by a `WebView`
(clicking a download link, or an explicit download request) is reported to
the view's `DownloadDelegate`, which decides where the file is saved. This
mirrors Apple's `WKDownloadDelegate`.

Downloads are wired through the web view's network session; the delegate is
installed per view.

## Rules

- Without a delegate (or with the default delegate), **every download is
  cancelled**: the default `decide_destination` returns `None`.
- The destination must be a full file path. Returning a path whose parent
  directory does not exist fails the download.
- All delegate methods run on the main thread.

## Constructors

### `WebView::set_download_delegate`

```rust
pub fn set_download_delegate(&self, delegate: Box<dyn DownloadDelegate>)
```

Installs the download delegate. Call it right after creating the view,
before any page can start a download.

## DownloadDelegate

```rust
pub trait DownloadDelegate {
    fn decide_destination(
        &mut self,
        download: &WebDownload,
        suggested_filename: &str,
    ) -> Option<String>;
    fn download_failed(&mut self, download: &WebDownload, error: &str) {}
    fn download_finished(&mut self, download: &WebDownload) {}
    fn download_progress(&mut self, download: &WebDownload) {}
}
```

| Method | Behavior |
|---|---|
| `decide_destination` | Return the full save path, or `None` to cancel |
| `download_failed` | Called with an error message when the download fails |
| `download_finished` | Called when the download completed successfully |
| `download_progress` | Called on every chunk of received data |

### `WebDownload`

| Method | Behavior |
|---|---|
| `uri(&self) -> Option<String>` | The URI being downloaded (`None` before the response arrives) |
| `destination(&self) -> Option<String>` | The chosen save path, if already decided |
| `estimated_progress(&self) -> f64` | Progress in `0.0..=1.0` |
| `received_bytes(&self) -> u64` | Bytes received so far |
| `cancel(&self)` | Cancels the download |

## Usage / Example

```rust,no_run
use std::path::PathBuf;
use webkit::{DefaultWebViewDelegate, DownloadDelegate, WebDownload, WebKitConfiguration, WebView};

struct SaveToDownloads {
    dir: PathBuf,
}

impl DownloadDelegate for SaveToDownloads {
    fn decide_destination(
        &mut self,
        _download: &WebDownload,
        suggested_filename: &str,
    ) -> Option<String> {
        let path = self.dir.join(suggested_filename);
        Some(path.to_string_lossy().into_owned())
    }

    fn download_finished(&mut self, _download: &WebDownload) {
        println!("download done");
    }
}

let web_view = WebView::new(WebKitConfiguration::new()).unwrap();
web_view.set_download_delegate(Box::new(SaveToDownloads { dir: PathBuf::from("/tmp") }));
```

## Cross References

- [WebView.md](WebView.md) -- creating views and installing delegates
- [Navigation.md](Navigation.md) -- navigation and policy decisions
