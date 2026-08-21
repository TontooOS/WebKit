//! Cookie management, the equivalent of `WKHTTPCookieStore` in Apple
//! WebKit.
//!
//! The [`CookieManager`] controls the accept policy (for example blocking
//! third-party cookies), reads and writes individual cookies and selects
//! the persistent cookie storage format. Obtain it from a view with
//! [`crate::WebView::cookie_manager`].

use webkit6 as wk;
use webkit6::prelude::*;

use crate::error::WebKitError;
use crate::web_view::WebView;

/// Which cookies the engine accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieAcceptPolicy {
    /// Accept every cookie.
    Always,
    /// Reject third-party cookies.
    NoThirdParty,
    /// Reject all cookies.
    Never,
}

impl CookieAcceptPolicy {
    fn to_engine(self) -> wk::CookieAcceptPolicy {
        match self {
            CookieAcceptPolicy::Always => wk::CookieAcceptPolicy::Always,
            CookieAcceptPolicy::NoThirdParty => wk::CookieAcceptPolicy::NoThirdParty,
            CookieAcceptPolicy::Never => wk::CookieAcceptPolicy::Never,
        }
    }

    fn from_engine(policy: wk::CookieAcceptPolicy) -> Self {
        match policy {
            wk::CookieAcceptPolicy::Never => CookieAcceptPolicy::Never,
            wk::CookieAcceptPolicy::NoThirdParty => CookieAcceptPolicy::NoThirdParty,
            _ => CookieAcceptPolicy::Always,
        }
    }
}

/// Persistent cookie storage format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieStorage {
    /// Human-readable text file.
    Text,
    /// SQLite database.
    Sqlite,
}

impl CookieStorage {
    fn to_engine(self) -> wk::CookiePersistentStorage {
        match self {
            CookieStorage::Text => wk::CookiePersistentStorage::Text,
            CookieStorage::Sqlite => wk::CookiePersistentStorage::Sqlite,
        }
    }
}

/// A single HTTP cookie.
#[derive(Debug, Clone)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie belongs to.
    pub domain: String,
    /// Path the cookie belongs to.
    pub path: String,
    /// Whether the cookie is only sent over secure connections.
    pub secure: bool,
    /// Whether the cookie is hidden from JavaScript (`HttpOnly`).
    pub http_only: bool,
}

impl Cookie {
    /// Create a session cookie (no expiry) for a domain and path.
    pub fn new(name: impl Into<String>, value: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            secure: false,
            http_only: false,
        }
    }

    /// Set the cookie path (default `/`).
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Mark the cookie as secure-only.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// Mark the cookie as `HttpOnly`.
    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }
}

/// The cookie store backing a web view's network session.
pub struct CookieManager {
    inner: wk::CookieManager,
}

impl CookieManager {
    /// Take the cookie manager of a web view's network session.
    ///
    /// Returns `None` when the engine has no session attached yet.
    pub fn from_view(view: &WebView) -> Option<Self> {
        view.inner()
            .network_session()
            .and_then(|session| session.cookie_manager())
            .map(|inner| Self { inner })
    }

    /// Set which cookies are accepted. Applies immediately to every
    /// request of the session.
    pub fn set_accept_policy(&self, policy: CookieAcceptPolicy) {
        self.inner.set_accept_policy(policy.to_engine());
    }

    /// The current accept policy.
    pub fn accept_policy(&self) -> Result<CookieAcceptPolicy, WebKitError> {
        let future = self.inner.accept_policy_future();
        let policy = block(future)?;
        Ok(CookieAcceptPolicy::from_engine(policy))
    }

    /// Every cookie in the store.
    pub fn all_cookies(&self) -> Result<Vec<Cookie>, WebKitError> {
        let future = self.inner.all_cookies_future();
        let mut cookies = block(future)?;
        Ok(cookies.iter_mut().map(cookie_from_soup).collect())
    }

    /// All cookies that would be sent for a URI.
    pub fn cookies_for_uri(&self, uri: &str) -> Result<Vec<Cookie>, WebKitError> {
        let future = self.inner.cookies_future(uri);
        let mut cookies = block(future)?;
        Ok(cookies.iter_mut().map(cookie_from_soup).collect())
    }

    /// Add or update a cookie in the store.
    pub fn add_cookie(&self, cookie: &Cookie) -> Result<(), WebKitError> {
        let mut soup_cookie = soup::Cookie::new(
            &cookie.name,
            &cookie.value,
            &cookie.domain,
            &cookie.path,
            -1,
        );
        soup_cookie.set_secure(cookie.secure);
        soup_cookie.set_http_only(cookie.http_only);
        let future = self.inner.add_cookie_future(&soup_cookie);
        block(future)
    }

    /// Delete the cookie matching domain, path and name.
    pub fn delete_cookie(
        &self,
        domain: &str,
        path: &str,
        name: &str,
    ) -> Result<(), WebKitError> {
        let cookie = soup::Cookie::new(name, "", domain, path, -1);
        let future = self.inner.delete_cookie_future(&cookie);
        block(future)
    }

    /// Store cookies persistently in the given file. Call before the
    /// first web view of the session is created for the setting to apply
    /// to every cookie.
    pub fn set_persistent_storage(&self, filename: &str, storage: CookieStorage) {
        self.inner.set_persistent_storage(filename, storage.to_engine());
    }
}

fn block<T>(future: impl std::future::Future<Output = Result<T, glib::Error>>) -> Result<T, WebKitError> {
    glib::MainContext::default()
        .block_on(future)
        .map_err(|e| WebKitError::Engine(e.to_string()))
}

fn cookie_from_soup(c: &mut soup::Cookie) -> Cookie {
    Cookie {
        name: c.name().unwrap_or_default().to_string(),
        value: c.value().unwrap_or_default().to_string(),
        domain: c.domain().unwrap_or_default().to_string(),
        path: c.path().unwrap_or_default().to_string(),
        secure: c.is_secure(),
        http_only: c.is_http_only(),
    }
}
