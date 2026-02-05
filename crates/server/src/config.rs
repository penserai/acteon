use serde::Deserialize;

/// Top-level configuration for the Acteon server, loaded from a TOML file.
#[derive(Debug, Deserialize)]
pub struct ActeonConfig {
    /// State backend configuration.
    #[serde(default)]
    pub state: StateConfig,
    /// Rule loading configuration.
    #[serde(default)]
    pub rules: RulesConfig,
    /// Executor configuration.
    #[serde(default)]
    pub executor: ExecutorConfig,
    /// HTTP server bind configuration.
    #[serde(default)]
    pub server: ServerConfig,
    /// Audit trail configuration.
    #[serde(default)]
    pub audit: AuditConfig,
    /// Authentication and authorization configuration.
    #[serde(default)]
    pub auth: AuthRefConfig,
    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: RateLimitRefConfig,
    /// Background processing configuration.
    #[serde(default)]
    pub background: BackgroundProcessingConfig,
}

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
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            url: None,
            prefix: None,
            region: None,
            table_name: None,
        }
    }
}

fn default_backend() -> String {
    "memory".to_owned()
}

/// Configuration for loading rules from disk.
#[derive(Debug, Default, Deserialize)]
pub struct RulesConfig {
    /// Optional path to a directory containing YAML rule files.
    pub directory: Option<String>,
}

/// Configuration for the action executor.
#[derive(Debug, Default, Deserialize)]
pub struct ExecutorConfig {
    /// Maximum retry attempts per action.
    pub max_retries: Option<u32>,
    /// Per-action execution timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Maximum number of actions executing concurrently.
    pub max_concurrent: Option<usize>,
    /// Whether to enable the dead-letter queue for failed actions.
    #[serde(default)]
    pub dlq_enabled: bool,
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            shutdown_timeout_seconds: default_shutdown_timeout(),
            external_url: None,
            approval_secret: None,
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

/// Configuration for the audit trail system.
#[derive(Debug, Deserialize)]
pub struct AuditConfig {
    /// Whether audit recording is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Which backend to use: `"memory"`, `"postgres"`, `"clickhouse"`, or `"elasticsearch"`.
    #[serde(default = "default_audit_backend")]
    pub backend: String,
    /// Connection URL for the audit backend (used by postgres).
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

/// Reference to the auth config file from `acteon.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct AuthRefConfig {
    /// Whether authentication/authorization is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the auth config file (`auth.toml`), relative to `acteon.toml` or absolute.
    pub config_path: Option<String>,
    /// Whether to watch the auth config file for changes (hot-reload). Defaults to `true`.
    pub watch: Option<bool>,
}

/// Reference to the rate limit config file from `acteon.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct RateLimitRefConfig {
    /// Whether rate limiting is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the rate limit config file (`ratelimit.toml`), relative to `acteon.toml` or absolute.
    pub config_path: Option<String>,
    /// Behavior when the state store is unavailable.
    #[serde(default)]
    pub on_error: RateLimitErrorBehavior,
}

/// Behavior when the rate limiter's state store is unavailable.
#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitErrorBehavior {
    /// Allow requests through (fail-open).
    #[default]
    Allow,
    /// Deny requests (fail-closed).
    Deny,
}

/// Configuration for background processing (group flushing, timeouts).
#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundProcessingConfig {
    /// Whether background processing is enabled.
    #[serde(default = "default_background_enabled")]
    pub enabled: bool,
    /// How often to check for ready groups (seconds).
    #[serde(default = "default_group_flush_interval")]
    pub group_flush_interval_seconds: u64,
    /// How often to check for state machine timeouts (seconds).
    #[serde(default = "default_timeout_check_interval")]
    pub timeout_check_interval_seconds: u64,
    /// How often to run cleanup tasks (seconds).
    #[serde(default = "default_cleanup_interval_bg")]
    pub cleanup_interval_seconds: u64,
    /// Whether to flush groups automatically.
    #[serde(default = "default_enable_group_flush")]
    pub enable_group_flush: bool,
    /// Whether to process state machine timeouts.
    #[serde(default = "default_enable_timeout_processing")]
    pub enable_timeout_processing: bool,
    /// Whether to retry failed approval notifications.
    #[serde(default = "default_enable_approval_retry")]
    pub enable_approval_retry: bool,
    /// Namespace to scan for timeouts (required for timeout processing).
    #[serde(default)]
    pub namespace: String,
    /// Tenant to scan for timeouts (required for timeout processing).
    #[serde(default)]
    pub tenant: String,
}

impl Default for BackgroundProcessingConfig {
    fn default() -> Self {
        Self {
            enabled: default_background_enabled(),
            group_flush_interval_seconds: default_group_flush_interval(),
            timeout_check_interval_seconds: default_timeout_check_interval(),
            cleanup_interval_seconds: default_cleanup_interval_bg(),
            enable_group_flush: default_enable_group_flush(),
            enable_timeout_processing: default_enable_timeout_processing(),
            enable_approval_retry: default_enable_approval_retry(),
            namespace: String::new(),
            tenant: String::new(),
        }
    }
}

fn default_background_enabled() -> bool {
    false
}

fn default_group_flush_interval() -> u64 {
    5
}

fn default_timeout_check_interval() -> u64 {
    10
}

fn default_cleanup_interval_bg() -> u64 {
    60
}

fn default_enable_group_flush() -> bool {
    true
}

fn default_enable_timeout_processing() -> bool {
    true
}

fn default_enable_approval_retry() -> bool {
    true
}
