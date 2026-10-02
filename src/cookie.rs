//! Cookie management, the equivalent of `WKHTTPCookieStore` in Apple
//! WebKit.
//!
//! [`Cookie`], [`CookieAcceptPolicy`] and [`CookieStorage`] are
//! backend-neutral. The out-of-process engine manages cookies in its
//! helper process.

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



/// Persistent cookie storage format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieStorage {
    /// Human-readable text file.
    Text,
    /// SQLite database.
    Sqlite,
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
    /// Expiry as unix seconds, or `None` for a session cookie that dies
    /// with the engine.
    pub expires: Option<i64>,
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
            expires: None,
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

    /// Set the expiry as unix seconds (`None` = session cookie).
    pub fn expires(mut self, expires: Option<i64>) -> Self {
        self.expires = expires;
        self
    }
}

impl Cookie {
    /// Render as a JSON object for engine IPC.
    pub fn to_json_value(&self) -> foundation::serialization::JsonValue {
        use foundation::serialization::JsonValue;
        JsonValue::Object(vec![
            ("name".to_string(), JsonValue::Str(self.name.clone())),
            ("value".to_string(), JsonValue::Str(self.value.clone())),
            ("domain".to_string(), JsonValue::Str(self.domain.clone())),
            ("path".to_string(), JsonValue::Str(self.path.clone())),
            ("secure".to_string(), JsonValue::Bool(self.secure)),
            ("httpOnly".to_string(), JsonValue::Bool(self.http_only)),
            (
                "expires".to_string(),
                match self.expires {
                    Some(expires) => JsonValue::Integer(expires),
                    None => JsonValue::Null,
                },
            ),
        ])
    }

    /// Parse from a JSON object (missing fields default like serde did).
    pub fn from_json_value(doc: &foundation::serialization::JsonValue) -> Self {
        let str_field = |key: &str| {
            doc.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let bool_field = |key: &str| {
            doc.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
        };
        Self {
            name: str_field("name"),
            value: str_field("value"),
            domain: str_field("domain"),
            path: {
                let path = str_field("path");
                if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                }
            },
            secure: bool_field("secure"),
            http_only: bool_field("httpOnly"),
            expires: doc.get("expires").and_then(|v| v.as_i64()),
        }
    }
}
