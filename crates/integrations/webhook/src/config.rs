use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// HTTP method to use for the webhook request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    /// Returns the method name as an uppercase string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// Authentication method for the webhook endpoint.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    /// HTTP Bearer token (`Authorization: Bearer <token>`).
    Bearer(String),

    /// HTTP Basic authentication (`Authorization: Basic <base64>`).
    Basic { username: String, password: String },

    /// API key sent in a custom header.
    ApiKey { header: String, value: String },

    /// HMAC signature of the request body, sent in a header.
    /// The signature is computed as `HMAC-SHA256(secret, body)` and
    /// hex-encoded.
    HmacSha256 { secret: String, header: String },
}

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer(_) => f.debug_tuple("Bearer").field(&"[REDACTED]").finish(),
            Self::Basic { username, .. } => f
                .debug_struct("Basic")
                .field("username", username)
                .field("password", &"[REDACTED]")
                .finish(),
            Self::ApiKey { header, .. } => f
                .debug_struct("ApiKey")
                .field("header", header)
                .field("value", &"[REDACTED]")
                .finish(),
            Self::HmacSha256 { header, .. } => f
                .debug_struct("HmacSha256")
                .field("secret", &"[REDACTED]")
                .field("header", header)
                .finish(),
        }
    }
}

/// Controls what is sent as the request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadMode {
    /// Send the full serialized [`Action`](acteon_core::Action) as the body.
    FullAction,
    /// Send only `action.payload` as the body.
    PayloadOnly,
}

/// Configuration for the webhook provider.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Target URL for the webhook.
    pub url: String,

    /// HTTP method (defaults to `POST`).
    pub method: HttpMethod,

    /// Authentication method, if any.
    pub auth: Option<AuthMethod>,

    /// Static headers to include in every request.
    pub headers: HashMap<String, String>,

    /// Controls what is sent as the request body.
    pub payload_mode: PayloadMode,

    /// Request timeout.
    pub timeout: Duration,

    /// HTTP status codes considered successful. If empty, any 2xx is
    /// accepted.
    pub success_status_codes: Vec<u16>,

    /// Whether to follow redirects.
    pub follow_redirects: bool,
}

impl WebhookConfig {
    /// Create a new configuration targeting the given URL.
    ///
    /// Defaults to `POST`, 30-second timeout, full-action payload, and
    /// accepting any 2xx status.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: HttpMethod::Post,
            auth: None,
            headers: HashMap::new(),
            payload_mode: PayloadMode::FullAction,
            timeout: Duration::from_secs(30),
            success_status_codes: Vec::new(),
            follow_redirects: true,
        }
    }

    /// Set the HTTP method.
    #[must_use]
    pub fn with_method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    /// Set the authentication method.
    #[must_use]
    pub fn with_auth(mut self, auth: AuthMethod) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Add a static header.
    #[must_use]
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the payload mode.
    #[must_use]
    pub fn with_payload_mode(mut self, mode: PayloadMode) -> Self {
        self.payload_mode = mode;
        self
    }

    /// Set the request timeout in seconds.
    #[must_use]
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }

    /// Set the request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set specific HTTP status codes to consider successful.
    ///
    /// When empty (the default), any 2xx status code is accepted.
    #[must_use]
    pub fn with_success_status_codes(mut self, codes: Vec<u16>) -> Self {
        self.success_status_codes = codes;
        self
    }

    /// Disable following HTTP redirects.
    #[must_use]
    pub fn with_no_redirects(mut self) -> Self {
        self.follow_redirects = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = WebhookConfig::new("https://example.com/hook");
        assert_eq!(config.url, "https://example.com/hook");
        assert_eq!(config.method, HttpMethod::Post);
        assert!(config.auth.is_none());
        assert!(config.headers.is_empty());
        assert_eq!(config.payload_mode, PayloadMode::FullAction);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert!(config.success_status_codes.is_empty());
        assert!(config.follow_redirects);
    }

    #[test]
    fn builder_methods() {
        let config = WebhookConfig::new("https://example.com")
            .with_method(HttpMethod::Put)
            .with_auth(AuthMethod::Bearer("tok".into()))
            .with_header("X-Custom", "val")
            .with_payload_mode(PayloadMode::PayloadOnly)
            .with_timeout_secs(10)
            .with_success_status_codes(vec![200, 201])
            .with_no_redirects();

        assert_eq!(config.method, HttpMethod::Put);
        assert!(config.auth.is_some());
        assert_eq!(config.headers.get("X-Custom").unwrap(), "val");
        assert_eq!(config.payload_mode, PayloadMode::PayloadOnly);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert_eq!(config.success_status_codes, vec![200, 201]);
        assert!(!config.follow_redirects);
    }

    #[test]
    fn http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn auth_method_serde_roundtrip() {
        let auth = AuthMethod::HmacSha256 {
            secret: "s3cret".into(),
            header: "X-Signature".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let back: AuthMethod = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AuthMethod::HmacSha256 { .. }));
    }

    #[test]
    fn payload_mode_serde_roundtrip() {
        let mode = PayloadMode::PayloadOnly;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"payload_only\"");
        let back: PayloadMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PayloadMode::PayloadOnly);
    }

    #[test]
    fn multiple_headers() {
        let config = WebhookConfig::new("https://example.com")
            .with_header("X-First", "one")
            .with_header("X-Second", "two")
            .with_header("X-Third", "three");
        assert_eq!(config.headers.len(), 3);
    }

    #[test]
    fn with_timeout_duration() {
        let config =
            WebhookConfig::new("https://example.com").with_timeout(Duration::from_millis(500));
        assert_eq!(config.timeout, Duration::from_millis(500));
    }

    #[test]
    fn auth_method_debug_redacts_secrets() {
        let bearer_value = "test-bearer-value-placeholder";
        let bearer = AuthMethod::Bearer(bearer_value.into());
        let debug = format!("{bearer:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(bearer_value));

        let pw = "test-pw-placeholder";
        let basic = AuthMethod::Basic {
            username: "user".into(),
            password: pw.into(),
        };
        let debug = format!("{basic:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("user"));
        assert!(!debug.contains(pw));

        let key_val = "test-key-placeholder";
        let api_key = AuthMethod::ApiKey {
            header: "X-Api-Key".into(),
            value: key_val.into(),
        };
        let debug = format!("{api_key:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("X-Api-Key"));
        assert!(!debug.contains(key_val));

        let hmac_val = "test-hmac-placeholder";
        let hmac = AuthMethod::HmacSha256 {
            secret: hmac_val.into(),
            header: "X-Signature".into(),
        };
        let debug = format!("{hmac:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("X-Signature"));
        assert!(!debug.contains(hmac_val));
    }
}
