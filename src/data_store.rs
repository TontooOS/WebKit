//! Website data storage, the equivalent of `WKWebsiteDataStore` in Apple
//! WebKit.
//!
//! [`WebsiteDataType`] is backend-neutral. The out-of-process engine owns
//! its data store in the helper process.

/// A subset of website data that can be inspected or cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// A snapshot of stored website data.
#[derive(Debug, Clone)]
pub struct WebsiteData {
    /// The data types present for this origin.
    pub types: WebsiteDataType,
    /// Estimated size in bytes.
    pub size: u64,
}