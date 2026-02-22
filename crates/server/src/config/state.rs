use serde::Deserialize;

/// Configuration for the state store backend.
#[derive(Debug, Deserialize)]
pub struct StateConfig {
    /// Which backend to use: `"memory"`, `"redis"`, `"postgres"`, `"dynamodb"`, or `"clickhouse"`.
    #[serde(default = "default_backend")]
    pub backend: String,

    /// Connection URL for the backend (e.g. `redis://localhost:6379`,
    /// `postgres://user:pass@localhost/acteon`).
    pub url: Option<String>,

    /// Key prefix for backends that support it. Defaults to `"acteon"`.
    pub prefix: Option<String>,

    /// AWS region for `DynamoDB` backend.
    pub region: Option<String>,

    /// `DynamoDB` table name.
    pub table_name: Option<String>,

    // ---- Postgres SSL fields ----
    /// SSL mode for `PostgreSQL` connections.
    #[serde(default)]
    pub ssl_mode: Option<String>,

    /// Path to the CA certificate for `PostgreSQL` SSL verification.
    #[serde(default)]
    pub ssl_root_cert: Option<String>,

    /// Path to the client certificate for `PostgreSQL` mTLS.
    #[serde(default)]
    pub ssl_cert: Option<String>,

    /// Path to the client private key for `PostgreSQL` mTLS.
    #[serde(default)]
    pub ssl_key: Option<String>,

    // ---- Redis TLS fields ----
    /// Whether to use TLS for Redis connections (uses `rediss://` scheme internally).
    #[serde(default)]
    pub tls_enabled: Option<bool>,

    /// Path to the CA certificate for Redis TLS verification.
    #[serde(default)]
    pub tls_ca_cert_path: Option<String>,

    /// Accept invalid certificates for Redis (dev/test only).
    #[serde(default)]
    pub tls_insecure: Option<bool>,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            url: None,
            prefix: None,
            region: None,
            table_name: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            tls_enabled: None,
            tls_ca_cert_path: None,
            tls_insecure: None,
        }
    }
}

fn default_backend() -> String {
    "memory".to_owned()
}
