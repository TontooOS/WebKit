//! Geolocation for TontooWebKit, backed by CoreLocation.
//!
//! There is no in-process engine to attach CoreLocation to; the
//! out-of-process WPE helper owns geolocation instead.
//!
//! Call it once before the first web view is created. Pages still need the
//! app to grant geolocation in
//! [`WebViewDelegate::permission_request`](crate::WebViewDelegate::permission_request);
//! attaching the provider never grants anything by itself.

use crate::error::WebKitError;

/// Feed engine geolocation from CoreLocation.
///
/// Always returns `Err`: there is no in-process engine to attach
/// CoreLocation to; the WPE helper owns geolocation instead.
pub fn attach_core_location() -> Result<(), WebKitError> {
    Err(WebKitError::Engine(
        "geolocation is owned by the WPE helper".into(),
    ))
}
