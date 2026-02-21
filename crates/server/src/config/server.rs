use serde::Deserialize;

/// A named HMAC key for signing/verifying approval URLs (config representation).
#[derive(Debug, Deserialize)]
pub struct ApprovalKeyConfig {
    /// Key identifier (e.g. `"k1"`, `"k2"`).
    pub id: String,
    /// Hex-encoded HMAC secret.
    pub secret: String,
}

/// HTTP server bind configuration.
#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Address to bind to.
    #[serde(default = "default_host")]
    pub host: String,
    /// Port to listen on.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Graceful shutdown timeout in seconds.
    ///
    /// This is the maximum time to wait for in-flight requests and pending
    /// audit tasks to complete during shutdown. Should be longer than any
    /// individual audit backend connection timeout.
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_seconds: u64,
    /// External URL for building approval links (e.g. `https://acteon.example.com`).
    ///
    /// If not set, defaults to `http://localhost:{port}`.
    pub external_url: Option<String>,
    /// Hex-encoded HMAC secret for signing approval URLs.
    ///
    /// If not set, a random secret is generated on startup (approval URLs
    /// will not survive server restarts).
    pub approval_secret: Option<String>,
    /// Named HMAC keys for signing/verifying approval URLs (multi-key).
    ///
    /// The first key is the current signing key. Additional keys are accepted
    /// during verification to support key rotation.
    /// Takes precedence over `approval_secret` when set.
    pub approval_keys: Option<Vec<ApprovalKeyConfig>>,
    /// Maximum concurrent SSE connections per tenant (default: 10).
    ///
    /// Limits resource exhaustion from long-lived SSE connections. Each
    /// tenant is tracked independently.
    pub max_sse_connections_per_tenant: Option<usize>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            shutdown_timeout_seconds: default_shutdown_timeout(),
            external_url: None,
            approval_secret: None,
            approval_keys: None,
            max_sse_connections_per_tenant: None,
        }
    }
}

fn default_shutdown_timeout() -> u64 {
    30
}

fn default_host() -> String {
    "127.0.0.1".to_owned()
}

fn default_port() -> u16 {
    8080
}

/// Admin UI configuration.
#[derive(Debug, Deserialize)]
pub struct UiConfig {
    /// Whether to serve the Admin UI.
    #[serde(default = "default_ui_enabled")]
    pub enabled: bool,
    /// Path to the directory containing the built Admin UI static files.
    /// Defaults to `"ui/dist"`.
    #[serde(default = "default_ui_dist")]
    pub dist_path: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: default_ui_enabled(),
            dist_path: default_ui_dist(),
        }
    }
}

fn default_ui_enabled() -> bool {
    true
}

fn default_ui_dist() -> String {
    "ui/dist".to_owned()
}
