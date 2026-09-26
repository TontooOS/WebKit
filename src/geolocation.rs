//! Geolocation for TontooWebKit, backed by CoreLocation.
//!
//! With the `gtk-backend` feature [`attach_core_location`] feeds positions
//! from CoreLocation (GPS / WiFi / IP providers) into the WebKitGTK engine
//! through its `GeolocationManager`. Without the feature it returns an
//! error; the out-of-process WPE helper will own geolocation instead.
//!
//! Call it once before the first web view is created. Pages still need the
//! app to grant geolocation in
//! [`WebViewDelegate::permission_request`](crate::WebViewDelegate::permission_request);
//! attaching the provider never grants anything by itself.

#[cfg(feature = "gtk-backend")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "gtk-backend")]
use std::thread;
#[cfg(feature = "gtk-backend")]
use std::time::Duration;

#[cfg(feature = "gtk-backend")]
use webkit6 as wk;

use crate::error::WebKitError;

#[cfg(feature = "gtk-backend")]
static ATTACHED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "gtk-backend")]
static ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "gtk-backend")]
static HIGH_ACCURACY: AtomicBool = AtomicBool::new(false);

/// Update cadence in seconds.
#[cfg(feature = "gtk-backend")]
const INTERVAL_NORMAL: u64 = 30;
#[cfg(feature = "gtk-backend")]
const INTERVAL_HIGH_ACCURACY: u64 = 2;

/// Feed engine geolocation from CoreLocation.
///
/// Idempotent: calling it more than once has no extra effect. Returns
/// `Err(WebKitError::Engine)` when the engine has no default web context
/// or no geolocation manager. Without the `gtk-backend` feature always
/// returns `Err` (the WPE helper will own geolocation instead).
#[cfg(feature = "gtk-backend")]
pub fn attach_core_location() -> Result<(), WebKitError> {
    if ATTACHED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let context = wk::WebContext::default()
        .ok_or_else(|| WebKitError::Engine("no default web context".into()))?;
    let manager = context
        .geolocation_manager()
        .ok_or_else(|| WebKitError::Engine("engine has no geolocation manager".into()))?;

    manager.connect_enable_high_accuracy_notify(|manager| {
        HIGH_ACCURACY.store(manager.enables_high_accuracy(), Ordering::SeqCst);
    });

    manager.connect_start(move |manager| {
        HIGH_ACCURACY.store(manager.enables_high_accuracy(), Ordering::SeqCst);
        // Only one worker thread even if a page starts several watches.
        if !ACTIVE.swap(true, Ordering::SeqCst) {
            thread::spawn(update_loop);
        }
        true
    });

    manager.connect_stop(|_manager| {
        ACTIVE.store(false, Ordering::SeqCst);
    });

    Ok(())
}

/// Stub without the `gtk-backend` feature: there is no in-process engine
/// to attach CoreLocation to.
#[cfg(not(feature = "gtk-backend"))]
pub fn attach_core_location() -> Result<(), WebKitError> {
    Err(WebKitError::Engine(
        "geolocation needs the gtk-backend feature or the WPE helper".into(),
    ))
}

#[cfg(feature = "gtk-backend")]
fn update_loop() {
    while ACTIVE.load(Ordering::SeqCst) {
        match corelocation::get_location() {
            Ok(location) => {
                let latitude = location.coordinates.latitude;
                let longitude = location.coordinates.longitude;
                let accuracy = location.accuracy.max(1.0);
                glib::MainContext::default().invoke(move || {
                    report_position(latitude, longitude, accuracy);
                });
            }
            Err(error) => {
                let message = error.to_string();
                glib::MainContext::default().invoke(move || {
                    report_failure(&message);
                });
            }
        }
        let interval = if HIGH_ACCURACY.load(Ordering::SeqCst) {
            INTERVAL_HIGH_ACCURACY
        } else {
            INTERVAL_NORMAL
        };
        // Sleep in small steps so `stop` reacts quickly.
        for _ in 0..interval * 10 {
            if !ACTIVE.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(feature = "gtk-backend")]
fn report_position(latitude: f64, longitude: f64, accuracy: f64) {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    if let Some(manager) = default_geolocation_manager() {
        let mut position = wk::GeolocationPosition::new(latitude, longitude, accuracy);
        manager.update_position(&mut position);
    }
}

#[cfg(feature = "gtk-backend")]
fn report_failure(message: &str) {
    if !ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    if let Some(manager) = default_geolocation_manager() {
        manager.failed(message);
    }
}

#[cfg(feature = "gtk-backend")]
fn default_geolocation_manager() -> Option<wk::GeolocationManager> {
    wk::WebContext::default()
        .and_then(|context| context.geolocation_manager())
}
