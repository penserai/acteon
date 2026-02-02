/// Configuration for the Elasticsearch audit store.
pub struct ElasticsearchAuditConfig {
    /// Elasticsearch base URL (e.g. `http://localhost:9200`).
    pub url: String,

    /// Index name prefix (e.g. `"acteon_"`).
    pub index_prefix: String,

    /// Optional username for basic authentication.
    pub username: Option<String>,

    /// Optional password for basic authentication.
    pub password: Option<String>,
}

impl ElasticsearchAuditConfig {
    /// Create a new configuration with the given URL and sensible defaults.
    ///
    /// Defaults:
    /// - `index_prefix`: `"acteon_"`
    /// - No authentication credentials
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            index_prefix: "acteon_".to_owned(),
            username: None,
            password: None,
        }
    }

    /// Set the index name prefix.
    #[must_use]
    pub fn with_index_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.index_prefix = prefix.into();
        self
    }

    /// Set basic authentication credentials.
    #[must_use]
    pub fn with_basic_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Return the full index name by combining the prefix with the `audit` suffix.
    pub fn index_name(&self) -> String {
        format!("{}audit", self.index_prefix)
    }
}
