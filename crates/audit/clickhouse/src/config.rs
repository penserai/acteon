/// Configuration for the `ClickHouse` audit store.
pub struct ClickHouseAuditConfig {
    /// `ClickHouse` HTTP endpoint URL (e.g. `http://localhost:8123`).
    pub url: String,

    /// Database name.
    pub database: String,

    /// Table name prefix (e.g. `"acteon_"`).
    pub prefix: String,

    /// Background cleanup interval in seconds.
    pub cleanup_interval_seconds: u64,
}

impl ClickHouseAuditConfig {
    /// Create a new configuration with the given URL and sensible defaults.
    ///
    /// Defaults:
    /// - `database`: `"default"`
    /// - `prefix`: `"acteon_"`
    /// - `cleanup_interval_seconds`: `3600`
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            database: "default".to_owned(),
            prefix: "acteon_".to_owned(),
            cleanup_interval_seconds: 3600,
        }
    }

    /// Set the database name.
    #[must_use]
    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = database.into();
        self
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
