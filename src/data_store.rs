//! Website data storage, the equivalent of `WKWebsiteDataStore` in Apple
//! WebKit.
//!
//! Controls where cookies, caches and storage live, enables private
//! (ephemeral) browsing, and clears stored website data.

use std::time::Duration;

use glib::translate::ToGlibPtr;
use serde::{Deserialize, Serialize};
use webkit6 as wk;

/// A subset of website data that can be inspected or cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebsiteDataType {
    pub memory_cache: bool,
    pub disk_cache: bool,
    pub offline_application_cache: bool,
    pub session_storage: bool,
    pub local_storage: bool,
    pub indexeddb_databases: bool,
    pub cookies: bool,
    pub device_id_hash_salt: bool,
    pub hsts_cache: bool,
    pub itp: bool,
    pub service_worker_registrations: bool,
    pub dom_cache: bool,
}

impl WebsiteDataType {
    /// Every supported data type.
    pub const fn all() -> Self {
        Self {
            memory_cache: true,
            disk_cache: true,
            offline_application_cache: true,
            session_storage: true,
            local_storage: true,
            indexeddb_databases: true,
            cookies: true,
            device_id_hash_salt: true,
            hsts_cache: true,
            itp: true,
            service_worker_registrations: true,
            dom_cache: true,
        }
    }

    /// No data types (useful as a starting point for a builder).
    pub const fn none() -> Self {
        Self {
            memory_cache: false,
            disk_cache: false,
            offline_application_cache: false,
            session_storage: false,
            local_storage: false,
            indexeddb_databases: false,
            cookies: false,
            device_id_hash_salt: false,
            hsts_cache: false,
            itp: false,
            service_worker_registrations: false,
            dom_cache: false,
        }
    }

    /// Cookies only.
    pub const fn cookies() -> Self {
        Self {
            cookies: true,
            ..Self::none()
        }
    }

    /// Caches only.
    pub const fn caches() -> Self {
        Self {
            memory_cache: true,
            disk_cache: true,
            offline_application_cache: true,
            dom_cache: true,
            ..Self::none()
        }
    }

    pub(crate) fn as_ffi(&self) -> webkit6_sys::WebKitWebsiteDataTypes {
        let mut bits = 0;
        if self.memory_cache {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_MEMORY_CACHE;
        }
        if self.disk_cache {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_DISK_CACHE;
        }
        if self.offline_application_cache {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_OFFLINE_APPLICATION_CACHE;
        }
        if self.session_storage {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_SESSION_STORAGE;
        }
        if self.local_storage {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_LOCAL_STORAGE;
        }
        if self.indexeddb_databases {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_INDEXEDDB_DATABASES;
        }
        if self.cookies {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_COOKIES;
        }
        if self.device_id_hash_salt {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_DEVICE_ID_HASH_SALT;
        }
        if self.hsts_cache {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_HSTS_CACHE;
        }
        if self.itp {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_ITP;
        }
        if self.service_worker_registrations {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_SERVICE_WORKER_REGISTRATIONS;
        }
        if self.dom_cache {
            bits |= webkit6_sys::WEBKIT_WEBSITE_DATA_DOM_CACHE;
        }
        bits
    }
}

/// A snapshot of stored website data.
#[derive(Debug, Clone)]
pub struct WebsiteData {
    /// The data types present for this origin.
    pub types: WebsiteDataType,
    /// Estimated size in bytes.
    pub size: u64,
}

/// The website data store backing a [`crate::WebView`].
#[derive(Debug, Clone)]
pub struct WebsiteDataStore {
    manager: wk::WebsiteDataManager,
}

impl WebsiteDataStore {
    /// The default persistent data store.
    pub fn default() -> Self {
        let manager = wk::WebsiteDataManager::builder().build();
        Self { manager }
    }

    /// A private, ephemeral data store. Nothing is written to disk and all
    /// data disappears when the process exits.
    pub fn ephemeral() -> Self {
        let manager = wk::WebsiteDataManager::builder().is_ephemeral(true).build();
        Self { manager }
    }

    /// A data store rooted at custom directories (useful for per-user or
    /// sandboxed apps).
    pub fn with_directories(
        data_directory: impl Into<String>,
        cache_directory: impl Into<String>,
    ) -> Self {
        let data = data_directory.into();
        let cache = cache_directory.into();
        let manager = wk::WebsiteDataManager::builder()
            .base_data_directory(data.as_str())
            .base_cache_directory(cache.as_str())
            .build();
        Self { manager }
    }

    /// Whether this store is ephemeral (private browsing).
    pub fn is_ephemeral(&self) -> bool {
        self.manager.is_ephemeral()
    }

    /// Clear website data of the given types within a time span.
    ///
    /// Blocks until the engine finishes clearing. Returns `Err` when the
    /// operation fails or is cancelled.
    pub fn clear(&self, types: WebsiteDataType, time_span: Duration) -> Result<(), crate::WebKitError> {
        let timespan = time_span.as_micros().min(i64::MAX as u128) as i64;
        self.clear_timespan(types, timespan)
    }

    /// Clear all website data for all time.
    pub fn clear_all(&self) -> Result<(), crate::WebKitError> {
        self.clear_timespan(WebsiteDataType::all(), -1)
    }

    /// Clear only cookies.
    pub fn clear_cookies(&self) -> Result<(), crate::WebKitError> {
        self.clear(WebsiteDataType::cookies(), Duration::from_secs(u64::MAX))
    }

    /// Clear only caches.
    pub fn clear_caches(&self) -> Result<(), crate::WebKitError> {
        self.clear(WebsiteDataType::caches(), Duration::from_secs(u64::MAX))
    }

    fn clear_timespan(&self, types: WebsiteDataType, timespan: i64) -> Result<(), crate::WebKitError> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let user_data = Box::into_raw(Box::new(tx));

        unsafe {
            webkit6_sys::webkit_website_data_manager_clear(
                self.manager.to_glib_none().0,
                types.as_ffi(),
                timespan,
                std::ptr::null_mut(),
                Some(clear_cb),
                user_data as glib::ffi::gpointer,
            );
        }

        match rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(crate::WebKitError::Engine(e)),
            Err(_) => Err(crate::WebKitError::Engine(
                "website data clear was cancelled".into(),
            )),
        }
    }
}

unsafe extern "C" fn clear_cb(
    _source: *mut glib::gobject_ffi::GObject,
    _res: *mut gio::ffi::GAsyncResult,
    user_data: glib::ffi::gpointer,
) {
    let sender: Box<std::sync::mpsc::Sender<Result<(), String>>> =
        Box::from_raw(user_data as *mut _);
    let _ = sender.send(Ok(()));
}