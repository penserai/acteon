use serde::Deserialize;

/// Configuration for the audit trail system.
#[derive(Debug, Deserialize)]
pub struct AuditConfig {
    /// Whether audit recording is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Which backend to use: `"memory"`, `"postgres"`, `"clickhouse"`, `"dynamodb"`,
    /// or `"elasticsearch"`.
    #[serde(default = "default_audit_backend")]
    pub backend: String,
    /// Connection URL for the audit backend (used by `postgres`, `clickhouse`, `elasticsearch`).
    pub url: Option<String>,
    /// Table prefix for the audit backend.
    #[serde(default = "default_audit_prefix")]
    pub prefix: String,
    /// TTL for audit records in seconds (default: 30 days).
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    /// Background cleanup interval in seconds (default: 1 hour).
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_seconds: u64,
    /// Whether to store action payloads in audit records.
    #[serde(default = "default_store_payload")]
    pub store_payload: bool,
    /// Field redaction configuration.
    #[serde(default)]
    pub redact: AuditRedactConfig,
    /// AWS region for the `DynamoDB` audit backend.
    #[serde(default)]
    pub region: Option<String>,
    /// `DynamoDB` table name for the audit backend.
    #[serde(default)]
    pub table_name: Option<String>,

    // ---- Postgres SSL fields ----
    /// SSL mode for `PostgreSQL` audit connections.
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
}

/// Configuration for redacting sensitive fields from audit payloads.
#[derive(Debug, Deserialize)]
pub struct AuditRedactConfig {
    /// Whether field redaction is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// List of field names or paths to redact (case-insensitive).
    ///
    /// Supports nested paths using dot notation (e.g., `"credentials.password"`).
    #[serde(default)]
    pub fields: Vec<String>,
    /// Placeholder text to replace redacted values with.
    #[serde(default = "default_redact_placeholder")]
    pub placeholder: String,
}

impl Default for AuditRedactConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fields: Vec::new(),
            placeholder: default_redact_placeholder(),
        }
    }
}

fn default_redact_placeholder() -> String {
    "[REDACTED]".to_owned()
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_audit_backend(),
            url: None,
            prefix: default_audit_prefix(),
            ttl_seconds: Some(2_592_000), // 30 days
            cleanup_interval_seconds: default_cleanup_interval(),
            store_payload: true,
            redact: AuditRedactConfig::default(),
            region: None,
            table_name: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        }
    }
}

fn default_audit_backend() -> String {
    "memory".to_owned()
}

fn default_audit_prefix() -> String {
    "acteon_".to_owned()
}

fn default_cleanup_interval() -> u64 {
    3600 // 1 hour
}

fn default_store_payload() -> bool {
    true
}
