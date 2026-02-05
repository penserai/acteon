/// Configuration for the Postgres audit store.
pub struct PostgresAuditConfig {
    /// Postgres connection URL.
    pub url: String,
    /// Table name prefix (e.g. "acteon_").
    pub prefix: String,
    /// Background cleanup interval in seconds.
    pub cleanup_interval_seconds: u64,
}

impl PostgresAuditConfig {
    /// Create a new configuration with the given URL and defaults.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            prefix: "acteon_".to_owned(),
            cleanup_interval_seconds: 3600,
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
}
