/// Configuration for the Postgres audit store.
pub struct PostgresAuditConfig {
    /// Postgres connection URL.
    pub url: String,
    /// Table name prefix (e.g. "acteon_").
    pub prefix: String,
    /// Background cleanup interval in seconds.
    pub cleanup_interval_seconds: u64,
    /// SSL mode (`disable`, `prefer`, `require`, `verify-ca`, `verify-full`).
    pub ssl_mode: Option<String>,
    /// Path to the CA certificate for SSL server verification.
    pub ssl_root_cert: Option<String>,
    /// Path to the client certificate for mTLS.
    pub ssl_cert: Option<String>,
    /// Path to the client private key for mTLS.
    pub ssl_key: Option<String>,
}

impl PostgresAuditConfig {
    /// Create a new configuration with the given URL and defaults.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            prefix: "acteon_".to_owned(),
            cleanup_interval_seconds: 3600,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        }
    }

    /// Set the table prefix.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the cleanup interval in seconds.
    #[must_use]
    pub fn with_cleanup_interval(mut self, seconds: u64) -> Self {
        self.cleanup_interval_seconds = seconds;
        self
    }

    /// Set the SSL mode.
    #[must_use]
    pub fn with_ssl_mode(mut self, mode: impl Into<String>) -> Self {
        self.ssl_mode = Some(mode.into());
        self
    }

    /// Set the SSL root certificate path.
    #[must_use]
    pub fn with_ssl_root_cert(mut self, path: impl Into<String>) -> Self {
        self.ssl_root_cert = Some(path.into());
        self
    }

    /// Set the client certificate path for mTLS.
    #[must_use]
    pub fn with_ssl_cert(mut self, path: impl Into<String>) -> Self {
        self.ssl_cert = Some(path.into());
        self
    }

    /// Set the client key path for mTLS.
    #[must_use]
    pub fn with_ssl_key(mut self, path: impl Into<String>) -> Self {
        self.ssl_key = Some(path.into());
        self
    }
}
