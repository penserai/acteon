use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
    /// Admin UI configuration.
    #[serde(default)]
    pub ui: UiConfig,
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
    /// LLM guardrail configuration.
    #[serde(default)]
    pub llm_guardrail: LlmGuardrailServerConfig,
    /// Task chain definitions.
    #[serde(default)]
    pub chains: ChainsConfig,
    /// Embedding provider configuration for semantic routing.
    #[serde(default)]
    pub embedding: EmbeddingServerConfig,
    /// Circuit breaker configuration for provider resilience.
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerServerConfig,
    /// OpenTelemetry distributed tracing configuration.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    /// Payload encryption at rest configuration.
    #[serde(default)]
    pub encryption: EncryptionConfig,
    /// Quota policy configuration.
    #[serde(default)]
    pub quotas: QuotaConfig,
    /// WASM plugin runtime configuration.
    #[serde(default)]
    pub wasm: WasmServerConfig,
    /// Compliance mode configuration (`SOC2` / `HIPAA`).
    #[serde(default)]
    pub compliance: ComplianceServerConfig,
    /// Payload template configuration.
    #[serde(default)]
    pub templates: TemplateServerConfig,
    /// Pre-dispatch enrichment configurations.
    ///
    /// Each entry describes a resource lookup to execute before rule evaluation,
    /// merging live external state into the action payload.
    #[serde(default)]
    pub enrichments: Vec<EnrichmentConfigToml>,
    /// Provider definitions.
    ///
    /// Each entry registers a named provider that actions can be routed to.
    /// Supported types: `"webhook"` (HTTP POST) and `"log"` (logs and returns
    /// success).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

/// Configuration for a single provider instance.
///
/// # Example
///
/// ```toml
/// [[providers]]
/// name = "email"
/// type = "webhook"
/// url = "http://localhost:9999/webhook"
///
/// [[providers]]
/// name = "slack"
/// type = "log"
/// ```
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    /// Unique name for this provider.
    pub name: String,
    /// Provider type: `"webhook"`, `"log"`, `"twilio"`, `"teams"`, `"discord"`,
    /// `"email"`, `"aws-sns"`, `"aws-lambda"`, `"aws-eventbridge"`, `"aws-sqs"`,
    /// `"aws-s3"`, `"aws-ec2"`, or `"aws-autoscaling"`.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Target URL (required for `"webhook"` type).
    pub url: Option<String>,
    /// Additional HTTP headers (used by `"webhook"` type).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Twilio Account SID (required for `"twilio"` type).
    pub account_sid: Option<String>,
    /// Twilio Auth Token (required for `"twilio"` type). Supports `ENC[...]`.
    pub auth_token: Option<String>,
    /// Default sender phone number (used by `"twilio"` type).
    pub from_number: Option<String>,
    /// Webhook URL (used by `"teams"` and `"discord"` types).
    pub webhook_url: Option<String>,
    /// Generic token field (future use).
    pub token: Option<String>,
    /// Default channel or recipient.
    pub default_channel: Option<String>,
    /// SMTP username for email authentication.
    pub username: Option<String>,
    /// SMTP authentication credential. Supports `ENC[...]`.
    #[serde(alias = "smtp_password")]
    pub password: Option<String>,

    // ---- Email provider fields ----
    /// Email backend: `"smtp"` (default) or `"ses"`.
    #[serde(default)]
    pub email_backend: Option<String>,
    /// SMTP server hostname (used by `"email"` type with SMTP backend).
    pub smtp_host: Option<String>,
    /// SMTP server port (used by `"email"` type with SMTP backend).
    pub smtp_port: Option<u16>,
    /// Sender email address (used by `"email"` type).
    pub from_address: Option<String>,
    /// Whether to use TLS for SMTP.
    #[serde(default)]
    pub tls: Option<bool>,

    // ---- AWS provider fields ----
    /// AWS region (used by all `"aws-*"` types and `"email"` with SES backend).
    pub aws_region: Option<String>,
    /// AWS IAM role ARN for STS assume-role (used by `"aws-*"` types).
    pub aws_role_arn: Option<String>,
    /// AWS endpoint URL override for `LocalStack` (used by `"aws-*"` types).
    pub aws_endpoint_url: Option<String>,
    /// SNS topic ARN (used by `"aws-sns"` type).
    pub topic_arn: Option<String>,
    /// Lambda function name or ARN (used by `"aws-lambda"` type).
    pub function_name: Option<String>,
    /// Lambda function qualifier (used by `"aws-lambda"` type).
    pub qualifier: Option<String>,
    /// `EventBridge` event bus name (used by `"aws-eventbridge"` type).
    pub event_bus_name: Option<String>,
    /// SQS queue URL (used by `"aws-sqs"` type).
    pub queue_url: Option<String>,
    /// SES configuration set name (used by `"email"` with SES backend).
    pub ses_configuration_set: Option<String>,
    /// STS session name for assume-role (used by `"aws-*"` types).
    pub aws_session_name: Option<String>,
    /// STS external ID for cross-account trust policies (used by `"aws-*"` types).
    pub aws_external_id: Option<String>,
    /// S3 bucket name (used by `"aws-s3"` type).
    pub bucket_name: Option<String>,
    /// S3 object key prefix (used by `"aws-s3"` type).
    pub object_prefix: Option<String>,
    /// Default security group IDs (used by `"aws-ec2"` type).
    #[serde(default)]
    pub default_security_group_ids: Option<Vec<String>>,
    /// Default subnet ID (used by `"aws-ec2"` type).
    pub default_subnet_id: Option<String>,
    /// Default key-pair name (used by `"aws-ec2"` type).
    pub default_key_name: Option<String>,
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
    /// Default IANA timezone for time-based rule conditions (e.g. `"US/Eastern"`).
    ///
    /// When set, `time.*` fields use this timezone unless a rule provides its
    /// own `timezone` override. If not set, UTC is used.
    pub default_timezone: Option<String>,
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
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Whether to process scheduled actions.
    #[serde(default)]
    pub enable_scheduled_actions: bool,
    /// How often to check for due scheduled actions (seconds).
    #[serde(default = "default_scheduled_check_interval")]
    pub scheduled_check_interval_seconds: u64,
    /// Whether to process recurring actions.
    #[serde(default)]
    pub enable_recurring_actions: bool,
    /// How often to check for due recurring actions (seconds).
    #[serde(default = "default_recurring_check_interval")]
    pub recurring_check_interval_seconds: u64,
    /// Maximum number of recurring actions per tenant.
    #[serde(default = "default_max_recurring_actions_per_tenant")]
    pub max_recurring_actions_per_tenant: usize,
    /// Whether to run the data retention reaper.
    #[serde(default)]
    pub enable_retention_reaper: bool,
    /// How often to run the data retention reaper (seconds).
    #[serde(default = "default_retention_check_interval")]
    pub retention_check_interval_seconds: u64,
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
            enable_scheduled_actions: false,
            scheduled_check_interval_seconds: default_scheduled_check_interval(),
            enable_recurring_actions: false,
            recurring_check_interval_seconds: default_recurring_check_interval(),
            max_recurring_actions_per_tenant: default_max_recurring_actions_per_tenant(),
            enable_retention_reaper: false,
            retention_check_interval_seconds: default_retention_check_interval(),
            namespace: String::new(),
            tenant: String::new(),
        }
    }
}

fn default_retention_check_interval() -> u64 {
    3600
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

fn default_scheduled_check_interval() -> u64 {
    5
}

fn default_recurring_check_interval() -> u64 {
    60
}

fn default_max_recurring_actions_per_tenant() -> usize {
    100
}

/// Configuration for the optional LLM guardrail.
///
/// # Secret management
///
/// The `api_key` field supports encrypted values. To avoid storing your
/// LLM API key in plain text:
///
/// 1. Set the `ACTEON_AUTH_KEY` environment variable to a hex-encoded
///    256-bit master key.
/// 2. Run `acteon-server encrypt` and paste your API key on stdin.
/// 3. Copy the `ENC[...]` output into `api_key` in your `acteon.toml`.
#[derive(Debug, Deserialize)]
pub struct LlmGuardrailServerConfig {
    /// Whether the LLM guardrail is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OpenAI-compatible API endpoint.
    #[serde(default = "default_llm_endpoint")]
    pub endpoint: String,
    /// Model to use.
    #[serde(default = "default_llm_model")]
    pub model: String,
    /// API key for authentication.
    ///
    /// Supports `ENC[...]` encrypted values (requires `ACTEON_AUTH_KEY`).
    /// Use `acteon-server encrypt` to generate encrypted values.
    #[serde(default)]
    pub api_key: String,
    /// System prompt / policy sent to the LLM.
    #[serde(default)]
    pub policy: String,
    /// Per-action-type policy overrides.
    ///
    /// Keys are action type strings, values are policy prompts. These take
    /// precedence over the global `policy` but are overridden by per-rule
    /// metadata `llm_policy` entries.
    #[serde(default)]
    pub policies: HashMap<String, String>,
    /// Whether to allow actions when the LLM is unreachable.
    #[serde(default = "default_llm_fail_open")]
    pub fail_open: bool,
    /// Request timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Temperature for LLM sampling.
    pub temperature: Option<f64>,
    /// Maximum tokens in the response.
    pub max_tokens: Option<u32>,
}

impl Default for LlmGuardrailServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_llm_endpoint(),
            model: default_llm_model(),
            api_key: String::new(),
            policy: String::new(),
            policies: HashMap::new(),
            fail_open: default_llm_fail_open(),
            timeout_seconds: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

fn default_llm_endpoint() -> String {
    "https://api.openai.com/v1/chat/completions".to_owned()
}

fn default_llm_model() -> String {
    "gpt-4o-mini".to_owned()
}

fn default_llm_fail_open() -> bool {
    true
}

/// Configuration for the embedding provider used by semantic routing.
///
/// # Secret management
///
/// The `api_key` field supports encrypted values. To avoid storing your
/// embedding API key in plain text:
///
/// 1. Set the `ACTEON_AUTH_KEY` environment variable to a hex-encoded
///    256-bit master key.
/// 2. Run `acteon-server encrypt` and paste your API key on stdin.
/// 3. Copy the `ENC[...]` output into `api_key` in your `acteon.toml`.
///
/// ```toml
/// [embedding]
/// enabled = true
/// api_key = "ENC[AES256-GCM,...]"
/// ```
#[derive(Debug, Deserialize)]
pub struct EmbeddingServerConfig {
    /// Whether the embedding provider is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OpenAI-compatible embeddings API endpoint.
    #[serde(default = "default_embedding_endpoint")]
    pub endpoint: String,
    /// Embedding model name.
    #[serde(default = "default_embedding_model")]
    pub model: String,
    /// API key for authentication.
    ///
    /// Supports `ENC[...]` encrypted values (requires `ACTEON_AUTH_KEY`).
    /// Use `acteon-server encrypt` to generate encrypted values.
    #[serde(default)]
    pub api_key: String,
    /// Request timeout in seconds.
    #[serde(default = "default_embedding_timeout")]
    pub timeout_seconds: u64,
    /// Whether to allow actions when the embedding API is unreachable.
    #[serde(default = "default_embedding_fail_open")]
    pub fail_open: bool,
    /// Maximum number of topic embeddings to cache.
    #[serde(default = "default_topic_cache_capacity")]
    pub topic_cache_capacity: u64,
    /// TTL in seconds for cached topic embeddings.
    #[serde(default = "default_topic_cache_ttl")]
    pub topic_cache_ttl_seconds: u64,
    /// Maximum number of text embeddings to cache.
    #[serde(default = "default_text_cache_capacity")]
    pub text_cache_capacity: u64,
    /// TTL in seconds for cached text embeddings.
    #[serde(default = "default_text_cache_ttl")]
    pub text_cache_ttl_seconds: u64,
}

impl Default for EmbeddingServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_embedding_endpoint(),
            model: default_embedding_model(),
            api_key: String::new(),
            timeout_seconds: default_embedding_timeout(),
            fail_open: default_embedding_fail_open(),
            topic_cache_capacity: default_topic_cache_capacity(),
            topic_cache_ttl_seconds: default_topic_cache_ttl(),
            text_cache_capacity: default_text_cache_capacity(),
            text_cache_ttl_seconds: default_text_cache_ttl(),
        }
    }
}

fn default_embedding_endpoint() -> String {
    "https://api.openai.com/v1/embeddings".to_owned()
}

fn default_embedding_model() -> String {
    "text-embedding-3-small".to_owned()
}

fn default_embedding_timeout() -> u64 {
    10
}

fn default_embedding_fail_open() -> bool {
    true
}

fn default_topic_cache_capacity() -> u64 {
    10_000
}

fn default_topic_cache_ttl() -> u64 {
    3600
}

fn default_text_cache_capacity() -> u64 {
    1_000
}

fn default_text_cache_ttl() -> u64 {
    60
}

/// Configuration for provider circuit breakers.
///
/// When enabled, circuit breakers track provider health and automatically
/// open the circuit when failure rates exceed the threshold, routing to
/// fallback providers during outages.
///
/// # Example
///
/// ```toml
/// [circuit_breaker]
/// enabled = true
/// failure_threshold = 5
/// success_threshold = 2
/// recovery_timeout_seconds = 60
///
/// [circuit_breaker.providers.email]
/// failure_threshold = 10
/// recovery_timeout_seconds = 120
/// fallback_provider = "webhook"
/// ```
#[derive(Debug, Deserialize)]
pub struct CircuitBreakerServerConfig {
    /// Whether circuit breakers are enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Default number of consecutive failures before opening the circuit.
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,
    /// Default number of consecutive successes in half-open state to close the circuit.
    #[serde(default = "default_cb_success_threshold")]
    pub success_threshold: u32,
    /// Default recovery timeout in seconds before transitioning from open to half-open.
    #[serde(default = "default_cb_recovery_timeout")]
    pub recovery_timeout_seconds: u64,
    /// Per-provider configuration overrides.
    #[serde(default)]
    pub providers: HashMap<String, CircuitBreakerProviderConfig>,
}

impl Default for CircuitBreakerServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_cb_failure_threshold(),
            success_threshold: default_cb_success_threshold(),
            recovery_timeout_seconds: default_cb_recovery_timeout(),
            providers: HashMap::new(),
        }
    }
}

fn default_cb_failure_threshold() -> u32 {
    5
}

fn default_cb_success_threshold() -> u32 {
    2
}

fn default_cb_recovery_timeout() -> u64 {
    60
}

/// Per-provider circuit breaker overrides.
#[derive(Debug, Deserialize)]
pub struct CircuitBreakerProviderConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: Option<u32>,
    /// Number of consecutive successes in half-open state to close the circuit.
    pub success_threshold: Option<u32>,
    /// Recovery timeout in seconds.
    pub recovery_timeout_seconds: Option<u64>,
    /// Fallback provider to route to when the circuit is open.
    pub fallback_provider: Option<String>,
}

/// Configuration for payload encryption at rest.
///
/// When enabled, action payloads stored in the state and audit backends are
/// encrypted using AES-256-GCM. Requires the `ACTEON_PAYLOAD_KEY` environment
/// variable to be set to a 32-byte key (hex or base64 encoded).
///
/// # Example
///
/// ```toml
/// [encryption]
/// enabled = true
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct EncryptionConfig {
    /// Whether payload encryption is enabled.
    #[serde(default)]
    pub enabled: bool,
}

/// Configuration for tenant quota policies.
#[derive(Debug, Deserialize)]
pub struct QuotaConfig {
    /// Whether quota enforcement is enabled.
    #[serde(default = "default_quotas_enabled")]
    pub enabled: bool,
    /// Default window for new quota policies (e.g., `"daily"`).
    #[serde(default)]
    pub default_window: Option<String>,
    /// Default overage behavior for new quota policies (e.g., `"block"`).
    #[serde(default)]
    pub default_overage_behavior: Option<String>,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            enabled: default_quotas_enabled(),
            default_window: None,
            default_overage_behavior: None,
        }
    }
}

fn default_quotas_enabled() -> bool {
    true
}

/// Configuration for payload templates.
#[derive(Debug, Default, Deserialize)]
pub struct TemplateServerConfig {
    /// Directory to scan for `.jinja` template files on startup.
    #[serde(default)]
    pub directory: Option<String>,
    /// Directory to scan for profile YAML files on startup.
    #[serde(default)]
    pub profiles_directory: Option<String>,
}

/// Configuration for the WASM plugin runtime.
///
/// When enabled, Acteon loads `.wasm` plugin files from the configured
/// directory and makes them available for use in rule conditions via the
/// `wasm()` predicate.
///
/// # Example
///
/// ```toml
/// [wasm]
/// enabled = true
/// plugin_dir = "/etc/acteon/plugins"
/// default_memory_limit_bytes = 16777216
/// default_timeout_ms = 100
/// ```
#[derive(Debug, Deserialize)]
pub struct WasmServerConfig {
    /// Whether the WASM plugin runtime is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Directory to scan for `.wasm` plugin files on startup.
    #[serde(default)]
    pub plugin_dir: Option<String>,
    /// Default memory limit for plugins in bytes (default: 16 MB).
    #[serde(default = "default_wasm_memory_limit")]
    pub default_memory_limit_bytes: u64,
    /// Default execution timeout for plugins in milliseconds (default: 100 ms).
    #[serde(default = "default_wasm_timeout_ms")]
    pub default_timeout_ms: u64,
}

impl Default for WasmServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugin_dir: None,
            default_memory_limit_bytes: default_wasm_memory_limit(),
            default_timeout_ms: default_wasm_timeout_ms(),
        }
    }
}

fn default_wasm_memory_limit() -> u64 {
    16 * 1024 * 1024 // 16 MB
}

fn default_wasm_timeout_ms() -> u64 {
    100
}

/// Compliance mode configuration for `SOC2` / `HIPAA` audit mode.
///
/// # Example
///
/// ```toml
/// [compliance]
/// mode = "soc2"       # "none", "soc2", or "hipaa"
/// sync_audit_writes = true
/// immutable_audit = false
/// hash_chain = true
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ComplianceServerConfig {
    /// Compliance mode preset: `"none"` (default), `"soc2"`, or `"hipaa"`.
    ///
    /// Each mode pre-configures sensible defaults that can be individually
    /// overridden by setting the other fields explicitly.
    #[serde(default)]
    pub mode: String,
    /// Override: whether audit writes must be synchronous.
    #[serde(default)]
    pub sync_audit_writes: Option<bool>,
    /// Override: whether audit records are immutable.
    #[serde(default)]
    pub immutable_audit: Option<bool>,
    /// Override: whether `SHA-256` hash chaining is enabled.
    #[serde(default)]
    pub hash_chain: Option<bool>,
}

impl ComplianceServerConfig {
    /// Convert this config into a [`acteon_core::ComplianceConfig`], applying
    /// mode presets first, then any explicit overrides.
    pub fn to_compliance_config(&self) -> acteon_core::ComplianceConfig {
        let mode = match self.mode.to_lowercase().as_str() {
            "soc2" => acteon_core::ComplianceMode::Soc2,
            "hipaa" => acteon_core::ComplianceMode::Hipaa,
            _ => acteon_core::ComplianceMode::None,
        };

        let mut config = acteon_core::ComplianceConfig::new(mode);

        if let Some(v) = self.sync_audit_writes {
            config = config.with_sync_audit_writes(v);
        }
        if let Some(v) = self.immutable_audit {
            config = config.with_immutable_audit(v);
        }
        if let Some(v) = self.hash_chain {
            config = config.with_hash_chain(v);
        }

        config
    }

    /// Returns `true` if any compliance feature is enabled.
    pub fn is_active(&self) -> bool {
        let config = self.to_compliance_config();
        config.mode != acteon_core::ComplianceMode::None
            || config.sync_audit_writes
            || config.immutable_audit
            || config.hash_chain
    }
}

/// Configuration for task chain definitions.
#[derive(Debug, Deserialize)]
pub struct ChainsConfig {
    /// List of chain definitions.
    #[serde(default)]
    pub definitions: Vec<ChainConfigToml>,
    /// Maximum number of chain steps advancing concurrently.
    #[serde(default = "default_max_concurrent_advances")]
    pub max_concurrent_advances: usize,
    /// TTL in seconds for completed/failed/cancelled chain state records.
    ///
    /// After a chain reaches a terminal status, the state record is kept for
    /// this duration for audit purposes before expiring. Defaults to 7 days.
    #[serde(default = "default_completed_chain_ttl")]
    pub completed_chain_ttl_seconds: u64,
}

impl Default for ChainsConfig {
    fn default() -> Self {
        Self {
            definitions: Vec::new(),
            max_concurrent_advances: default_max_concurrent_advances(),
            completed_chain_ttl_seconds: default_completed_chain_ttl(),
        }
    }
}

fn default_max_concurrent_advances() -> usize {
    16
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn default_completed_chain_ttl() -> u64 {
    604_800 // 7 days
}

/// A single chain definition loaded from TOML.
#[derive(Debug, Deserialize)]
pub struct ChainConfigToml {
    /// Unique name for the chain.
    pub name: String,
    /// Ordered steps in the chain.
    pub steps: Vec<ChainStepConfigToml>,
    /// Failure policy: `"abort"` (default) or `"abort_no_dlq"`.
    pub on_failure: Option<String>,
    /// Optional timeout in seconds for the entire chain.
    pub timeout_seconds: Option<u64>,
    /// Optional notification target dispatched when the chain is cancelled.
    pub on_cancel: Option<ChainNotificationTargetToml>,
}

/// Notification target for chain cancellation events (TOML representation).
#[derive(Debug, Deserialize)]
pub struct ChainNotificationTargetToml {
    /// Provider to dispatch the notification through.
    pub provider: String,
    /// Action type for the notification action.
    pub action_type: String,
}

/// A single step in a chain definition loaded from TOML.
#[derive(Debug, Deserialize)]
pub struct ChainStepConfigToml {
    /// Step name (used for `{{steps.NAME.*}}` template references).
    pub name: String,
    /// Provider to execute this step with.
    /// Optional for sub-chain steps that invoke another chain instead of a provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Action type for the synthetic action.
    /// Optional for sub-chain steps.
    #[serde(default)]
    pub action_type: Option<String>,
    /// JSON payload template with `{{...}}` placeholders.
    #[serde(default = "default_empty_object")]
    pub payload_template: serde_json::Value,
    /// Per-step failure policy override: `"abort"`, `"skip"`, or `"dlq"`.
    pub on_failure: Option<String>,
    /// Optional delay in seconds before executing this step.
    pub delay_seconds: Option<u64>,
    /// Conditional branch conditions evaluated after this step completes.
    #[serde(default)]
    pub branches: Vec<BranchConditionToml>,
    /// Default next step name when no branch condition matches.
    #[serde(default)]
    pub default_next: Option<String>,
    /// Name of another chain to invoke as a sub-chain.
    /// Mutually exclusive with `provider`.
    #[serde(default)]
    pub sub_chain: Option<String>,
}

/// A branch condition in a chain step, loaded from TOML.
#[derive(Debug, Deserialize)]
pub struct BranchConditionToml {
    /// The field to evaluate (e.g., `"success"`, `"body.status"`).
    pub field: String,
    /// Comparison operator: `"eq"`, `"neq"`, `"contains"`, `"exists"`,
    /// `"gt"`, `"lt"`, `"gte"`, or `"lte"`.
    pub operator: String,
    /// Value to compare against (ignored for `"exists"`).
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// Name of the target step to jump to when this condition matches.
    pub target: String,
}

/// Configuration for `OpenTelemetry` distributed tracing.
///
/// When enabled, Acteon exports trace spans via OTLP to a collector (Jaeger,
/// Grafana Tempo, etc.), providing end-to-end visibility through the dispatch
/// pipeline: HTTP ingress, rule evaluation, state operations, provider
/// execution, and audit recording.
///
/// # Example
///
/// ```toml
/// [telemetry]
/// enabled = true
/// endpoint = "http://localhost:4317"
/// service_name = "acteon"
/// sample_ratio = 1.0
/// protocol = "grpc"
/// ```
#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    /// Whether `OpenTelemetry` tracing is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OTLP exporter endpoint.
    #[serde(default = "default_otel_endpoint")]
    pub endpoint: String,
    /// Service name reported in traces.
    #[serde(default = "default_otel_service_name")]
    pub service_name: String,
    /// Sampling ratio (0.0 to 1.0). `1.0` traces every request.
    #[serde(default = "default_otel_sample_ratio")]
    pub sample_ratio: f64,
    /// OTLP transport protocol: `"grpc"` or `"http"`.
    #[serde(default = "default_otel_protocol")]
    pub protocol: String,
    /// Exporter timeout in seconds.
    #[serde(default = "default_otel_timeout")]
    pub timeout_seconds: u64,
    /// Additional resource attributes as `key=value` pairs.
    #[serde(default)]
    pub resource_attributes: HashMap<String, String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_otel_endpoint(),
            service_name: default_otel_service_name(),
            sample_ratio: default_otel_sample_ratio(),
            protocol: default_otel_protocol(),
            timeout_seconds: default_otel_timeout(),
            resource_attributes: HashMap::new(),
        }
    }
}

fn default_otel_endpoint() -> String {
    "http://localhost:4317".to_owned()
}

fn default_otel_service_name() -> String {
    "acteon".to_owned()
}

fn default_otel_sample_ratio() -> f64 {
    1.0
}

fn default_otel_protocol() -> String {
    "grpc".to_owned()
}

fn default_otel_timeout() -> u64 {
    10
}

// ---------------------------------------------------------------------------
// ConfigSnapshot: a sanitized, serializable view of the server configuration.
// ---------------------------------------------------------------------------

/// Truncate a string to at most `max` characters, appending `"..."` if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Sanitized view of the full server configuration.
///
/// All secrets (API keys, HMAC secrets, approval keys) are masked so that
/// the snapshot is safe to expose via the admin API.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConfigSnapshot {
    /// Server bind and networking settings.
    pub server: ServerSnapshot,
    /// Admin UI configuration.
    pub ui: UiSnapshot,
    /// State backend configuration.
    pub state: StateSnapshot,
    /// Executor settings.
    pub executor: ExecutorSnapshot,
    /// Rule loading configuration.
    pub rules: RulesSnapshot,
    /// Audit trail configuration.
    pub audit: AuditSnapshot,
    /// Authentication configuration.
    pub auth: AuthSnapshot,
    /// Rate limiting configuration.
    pub rate_limit: RateLimitSnapshot,
    /// LLM guardrail configuration.
    pub llm_guardrail: LlmGuardrailSnapshot,
    /// Embedding provider configuration.
    pub embedding: EmbeddingSnapshot,
    /// Circuit breaker configuration.
    pub circuit_breaker: CircuitBreakerSnapshot,
    /// Background processing configuration.
    pub background: BackgroundSnapshot,
    /// Telemetry / `OpenTelemetry` configuration.
    pub telemetry: TelemetrySnapshot,
    /// Task chain configuration.
    pub chains: ChainsSnapshot,
    /// Payload encryption configuration.
    pub encryption: EncryptionSnapshot,
    /// WASM plugin runtime configuration.
    pub wasm: WasmSnapshot,
    /// Compliance mode configuration.
    pub compliance: ComplianceSnapshot,
    /// Registered provider summaries.
    pub providers: Vec<ProviderSnapshot>,
}

impl From<&ActeonConfig> for ConfigSnapshot {
    fn from(cfg: &ActeonConfig) -> Self {
        Self {
            server: ServerSnapshot::from(&cfg.server),
            ui: UiSnapshot::from(&cfg.ui),
            state: StateSnapshot::from(&cfg.state),
            executor: ExecutorSnapshot::from(&cfg.executor),
            rules: RulesSnapshot::from(&cfg.rules),
            audit: AuditSnapshot::from(&cfg.audit),
            auth: AuthSnapshot::from(&cfg.auth),
            rate_limit: RateLimitSnapshot::from(&cfg.rate_limit),
            llm_guardrail: LlmGuardrailSnapshot::from(&cfg.llm_guardrail),
            embedding: EmbeddingSnapshot::from(&cfg.embedding),
            circuit_breaker: CircuitBreakerSnapshot::from(&cfg.circuit_breaker),
            background: BackgroundSnapshot::from(&cfg.background),
            telemetry: TelemetrySnapshot::from(&cfg.telemetry),
            chains: ChainsSnapshot::from(&cfg.chains),
            encryption: EncryptionSnapshot::from(&cfg.encryption),
            wasm: WasmSnapshot::from(&cfg.wasm),
            compliance: ComplianceSnapshot::from(&cfg.compliance),
            providers: cfg.providers.iter().map(ProviderSnapshot::from).collect(),
        }
    }
}

/// Sanitized server bind configuration (secrets removed).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServerSnapshot {
    /// Bind host.
    pub host: String,
    /// Bind port.
    pub port: u16,
    /// Graceful shutdown timeout in seconds.
    pub shutdown_timeout_seconds: u64,
    /// External URL for approval links.
    pub external_url: Option<String>,
    /// Maximum concurrent SSE connections per tenant.
    pub max_sse_connections_per_tenant: Option<usize>,
}

impl From<&ServerConfig> for ServerSnapshot {
    fn from(cfg: &ServerConfig) -> Self {
        Self {
            host: cfg.host.clone(),
            port: cfg.port,
            shutdown_timeout_seconds: cfg.shutdown_timeout_seconds,
            external_url: cfg.external_url.clone(),
            max_sse_connections_per_tenant: cfg.max_sse_connections_per_tenant,
        }
    }
}

/// Sanitized Admin UI configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UiSnapshot {
    /// Whether the UI is enabled.
    pub enabled: bool,
    /// Path to the UI static files.
    pub dist_path: String,
}

impl From<&UiConfig> for UiSnapshot {
    fn from(cfg: &UiConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            dist_path: cfg.dist_path.clone(),
        }
    }
}

/// Sanitized state backend configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StateSnapshot {
    /// Backend type.
    pub backend: String,
    /// Whether a connection URL is configured.
    pub has_url: bool,
    /// Key prefix.
    pub prefix: Option<String>,
    /// AWS region (for `DynamoDB`).
    pub region: Option<String>,
    /// `DynamoDB` table name.
    pub table_name: Option<String>,
}

impl From<&StateConfig> for StateSnapshot {
    fn from(cfg: &StateConfig) -> Self {
        Self {
            backend: cfg.backend.clone(),
            has_url: cfg.url.is_some(),
            prefix: cfg.prefix.clone(),
            region: cfg.region.clone(),
            table_name: cfg.table_name.clone(),
        }
    }
}

/// Sanitized executor configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecutorSnapshot {
    /// Maximum retry attempts per action.
    pub max_retries: Option<u32>,
    /// Per-action execution timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Maximum number of concurrent executions.
    pub max_concurrent: Option<usize>,
    /// Whether the dead-letter queue is enabled.
    pub dlq_enabled: bool,
}

impl From<&ExecutorConfig> for ExecutorSnapshot {
    fn from(cfg: &ExecutorConfig) -> Self {
        Self {
            max_retries: cfg.max_retries,
            timeout_seconds: cfg.timeout_seconds,
            max_concurrent: cfg.max_concurrent,
            dlq_enabled: cfg.dlq_enabled,
        }
    }
}

/// Sanitized rules configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RulesSnapshot {
    /// Directory path for rule files.
    pub directory: Option<String>,
    /// Default IANA timezone for time-based conditions.
    pub default_timezone: Option<String>,
}

impl From<&RulesConfig> for RulesSnapshot {
    fn from(cfg: &RulesConfig) -> Self {
        Self {
            directory: cfg.directory.clone(),
            default_timezone: cfg.default_timezone.clone(),
        }
    }
}

/// Sanitized audit trail configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditSnapshot {
    /// Whether audit is enabled.
    pub enabled: bool,
    /// Backend type.
    pub backend: String,
    /// Whether a connection URL is configured.
    pub has_url: bool,
    /// Table prefix.
    pub prefix: String,
    /// TTL for audit records in seconds.
    pub ttl_seconds: Option<u64>,
    /// Cleanup interval in seconds.
    pub cleanup_interval_seconds: u64,
    /// Whether action payloads are stored.
    pub store_payload: bool,
    /// Redaction configuration.
    pub redact: AuditRedactSnapshot,
}

impl From<&AuditConfig> for AuditSnapshot {
    fn from(cfg: &AuditConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            backend: cfg.backend.clone(),
            has_url: cfg.url.is_some(),
            prefix: cfg.prefix.clone(),
            ttl_seconds: cfg.ttl_seconds,
            cleanup_interval_seconds: cfg.cleanup_interval_seconds,
            store_payload: cfg.store_payload,
            redact: AuditRedactSnapshot::from(&cfg.redact),
        }
    }
}

/// Sanitized audit redaction configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditRedactSnapshot {
    /// Whether redaction is enabled.
    pub enabled: bool,
    /// Number of redacted field patterns.
    pub field_count: usize,
    /// Redaction placeholder text.
    pub placeholder: String,
}

impl From<&AuditRedactConfig> for AuditRedactSnapshot {
    fn from(cfg: &AuditRedactConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            field_count: cfg.fields.len(),
            placeholder: cfg.placeholder.clone(),
        }
    }
}

/// Sanitized auth configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuthSnapshot {
    /// Whether auth is enabled.
    pub enabled: bool,
    /// Path to the auth config file.
    pub config_path: Option<String>,
    /// Whether file watching is enabled.
    pub watch: Option<bool>,
}

impl From<&AuthRefConfig> for AuthSnapshot {
    fn from(cfg: &AuthRefConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            config_path: cfg.config_path.clone(),
            watch: cfg.watch,
        }
    }
}

/// Sanitized rate limiting configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RateLimitSnapshot {
    /// Whether rate limiting is enabled.
    pub enabled: bool,
    /// Path to the rate limit config file.
    pub config_path: Option<String>,
    /// Error behavior when the state store is unavailable.
    pub on_error: RateLimitErrorBehavior,
}

impl From<&RateLimitRefConfig> for RateLimitSnapshot {
    fn from(cfg: &RateLimitRefConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            config_path: cfg.config_path.clone(),
            on_error: cfg.on_error,
        }
    }
}

/// Sanitized LLM guardrail configuration (API key masked).
#[derive(Debug, Clone, Default, Serialize)]
pub struct LlmGuardrailSnapshot {
    /// Whether the LLM guardrail is enabled.
    pub enabled: bool,
    /// API endpoint.
    pub endpoint: String,
    /// Model name.
    pub model: String,
    /// Whether an API key is configured.
    pub has_api_key: bool,
    /// Truncated global policy (first 100 characters).
    pub policy: String,
    /// Action type keys that have policy overrides.
    pub policy_keys: Vec<String>,
    /// Whether to fail open on LLM errors.
    pub fail_open: bool,
    /// Request timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Maximum response tokens.
    pub max_tokens: Option<u32>,
}

impl From<&LlmGuardrailServerConfig> for LlmGuardrailSnapshot {
    fn from(cfg: &LlmGuardrailServerConfig) -> Self {
        let mut keys: Vec<String> = cfg.policies.keys().cloned().collect();
        keys.sort();
        Self {
            enabled: cfg.enabled,
            endpoint: cfg.endpoint.clone(),
            model: cfg.model.clone(),
            has_api_key: !cfg.api_key.is_empty(),
            policy: truncate(&cfg.policy, 100),
            policy_keys: keys,
            fail_open: cfg.fail_open,
            timeout_seconds: cfg.timeout_seconds,
            temperature: cfg.temperature,
            max_tokens: cfg.max_tokens,
        }
    }
}

/// Sanitized embedding provider configuration (API key masked).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EmbeddingSnapshot {
    /// Whether the embedding provider is enabled.
    pub enabled: bool,
    /// API endpoint.
    pub endpoint: String,
    /// Model name.
    pub model: String,
    /// Whether an API key is configured.
    pub has_api_key: bool,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
    /// Whether to fail open on provider errors.
    pub fail_open: bool,
    /// Maximum topic embeddings to cache.
    pub topic_cache_capacity: u64,
    /// Topic cache TTL in seconds.
    pub topic_cache_ttl_seconds: u64,
    /// Maximum text embeddings to cache.
    pub text_cache_capacity: u64,
    /// Text cache TTL in seconds.
    pub text_cache_ttl_seconds: u64,
}

impl From<&EmbeddingServerConfig> for EmbeddingSnapshot {
    fn from(cfg: &EmbeddingServerConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            endpoint: cfg.endpoint.clone(),
            model: cfg.model.clone(),
            has_api_key: !cfg.api_key.is_empty(),
            timeout_seconds: cfg.timeout_seconds,
            fail_open: cfg.fail_open,
            topic_cache_capacity: cfg.topic_cache_capacity,
            topic_cache_ttl_seconds: cfg.topic_cache_ttl_seconds,
            text_cache_capacity: cfg.text_cache_capacity,
            text_cache_ttl_seconds: cfg.text_cache_ttl_seconds,
        }
    }
}

/// Sanitized circuit breaker configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CircuitBreakerSnapshot {
    /// Whether circuit breakers are enabled.
    pub enabled: bool,
    /// Default failure threshold.
    pub failure_threshold: u32,
    /// Default success threshold.
    pub success_threshold: u32,
    /// Default recovery timeout in seconds.
    pub recovery_timeout_seconds: u64,
    /// Per-provider override names.
    pub provider_overrides: Vec<String>,
}

impl From<&CircuitBreakerServerConfig> for CircuitBreakerSnapshot {
    fn from(cfg: &CircuitBreakerServerConfig) -> Self {
        let mut overrides: Vec<String> = cfg.providers.keys().cloned().collect();
        overrides.sort();
        Self {
            enabled: cfg.enabled,
            failure_threshold: cfg.failure_threshold,
            success_threshold: cfg.success_threshold,
            recovery_timeout_seconds: cfg.recovery_timeout_seconds,
            provider_overrides: overrides,
        }
    }
}

/// Sanitized background processing configuration.
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackgroundSnapshot {
    /// Whether background processing is enabled.
    pub enabled: bool,
    /// Group flush interval in seconds.
    pub group_flush_interval_seconds: u64,
    /// Timeout check interval in seconds.
    pub timeout_check_interval_seconds: u64,
    /// Cleanup interval in seconds.
    pub cleanup_interval_seconds: u64,
    /// Whether group flushing is enabled.
    pub enable_group_flush: bool,
    /// Whether timeout processing is enabled.
    pub enable_timeout_processing: bool,
    /// Whether approval retries are enabled.
    pub enable_approval_retry: bool,
    /// Whether scheduled actions are enabled.
    pub enable_scheduled_actions: bool,
    /// Scheduled action check interval in seconds.
    pub scheduled_check_interval_seconds: u64,
    /// Whether recurring actions are enabled.
    pub enable_recurring_actions: bool,
    /// Recurring action check interval in seconds.
    pub recurring_check_interval_seconds: u64,
    /// Maximum number of recurring actions per tenant.
    pub max_recurring_actions_per_tenant: usize,
    /// Whether the retention reaper is enabled.
    pub enable_retention_reaper: bool,
    /// Retention reaper check interval in seconds.
    pub retention_check_interval_seconds: u64,
}

impl Default for BackgroundSnapshot {
    fn default() -> Self {
        let cfg = BackgroundProcessingConfig::default();
        Self::from(&cfg)
    }
}

impl From<&BackgroundProcessingConfig> for BackgroundSnapshot {
    fn from(cfg: &BackgroundProcessingConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            group_flush_interval_seconds: cfg.group_flush_interval_seconds,
            timeout_check_interval_seconds: cfg.timeout_check_interval_seconds,
            cleanup_interval_seconds: cfg.cleanup_interval_seconds,
            enable_group_flush: cfg.enable_group_flush,
            enable_timeout_processing: cfg.enable_timeout_processing,
            enable_approval_retry: cfg.enable_approval_retry,
            enable_scheduled_actions: cfg.enable_scheduled_actions,
            scheduled_check_interval_seconds: cfg.scheduled_check_interval_seconds,
            enable_recurring_actions: cfg.enable_recurring_actions,
            recurring_check_interval_seconds: cfg.recurring_check_interval_seconds,
            max_recurring_actions_per_tenant: cfg.max_recurring_actions_per_tenant,
            enable_retention_reaper: cfg.enable_retention_reaper,
            retention_check_interval_seconds: cfg.retention_check_interval_seconds,
        }
    }
}

/// Sanitized telemetry configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TelemetrySnapshot {
    /// Whether `OpenTelemetry` tracing is enabled.
    pub enabled: bool,
    /// OTLP endpoint.
    pub endpoint: String,
    /// Service name.
    pub service_name: String,
    /// Sampling ratio (0.0 to 1.0).
    pub sample_ratio: f64,
    /// Transport protocol.
    pub protocol: String,
    /// Exporter timeout in seconds.
    pub timeout_seconds: u64,
    /// Resource attribute keys (values omitted for brevity).
    pub resource_attribute_keys: Vec<String>,
}

impl From<&TelemetryConfig> for TelemetrySnapshot {
    fn from(cfg: &TelemetryConfig) -> Self {
        let mut keys: Vec<String> = cfg.resource_attributes.keys().cloned().collect();
        keys.sort();
        Self {
            enabled: cfg.enabled,
            endpoint: cfg.endpoint.clone(),
            service_name: cfg.service_name.clone(),
            sample_ratio: cfg.sample_ratio,
            protocol: cfg.protocol.clone(),
            timeout_seconds: cfg.timeout_seconds,
            resource_attribute_keys: keys,
        }
    }
}

/// Sanitized task chain configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChainsSnapshot {
    /// Maximum concurrent chain step advances.
    pub max_concurrent_advances: usize,
    /// TTL in seconds for completed chain state records.
    pub completed_chain_ttl_seconds: u64,
    /// Chain definition summaries.
    pub definitions: Vec<ChainDefinitionSnapshot>,
}

impl From<&ChainsConfig> for ChainsSnapshot {
    fn from(cfg: &ChainsConfig) -> Self {
        Self {
            max_concurrent_advances: cfg.max_concurrent_advances,
            completed_chain_ttl_seconds: cfg.completed_chain_ttl_seconds,
            definitions: cfg
                .definitions
                .iter()
                .map(ChainDefinitionSnapshot::from)
                .collect(),
        }
    }
}

/// Summary of a single chain definition.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChainDefinitionSnapshot {
    /// Chain name.
    pub name: String,
    /// Number of steps in the chain.
    pub steps_count: usize,
    /// Overall timeout in seconds.
    pub timeout_seconds: Option<u64>,
}

impl From<&ChainConfigToml> for ChainDefinitionSnapshot {
    fn from(cfg: &ChainConfigToml) -> Self {
        Self {
            name: cfg.name.clone(),
            steps_count: cfg.steps.len(),
            timeout_seconds: cfg.timeout_seconds,
        }
    }
}

/// Sanitized encryption configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EncryptionSnapshot {
    /// Whether payload encryption is enabled.
    pub enabled: bool,
}

impl From<&EncryptionConfig> for EncryptionSnapshot {
    fn from(cfg: &EncryptionConfig) -> Self {
        Self {
            enabled: cfg.enabled,
        }
    }
}

/// Sanitized WASM plugin runtime configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WasmSnapshot {
    /// Whether the WASM runtime is enabled.
    pub enabled: bool,
    /// Plugin directory path.
    pub plugin_dir: Option<String>,
    /// Default memory limit in bytes.
    pub default_memory_limit_bytes: u64,
    /// Default timeout in milliseconds.
    pub default_timeout_ms: u64,
}

impl From<&WasmServerConfig> for WasmSnapshot {
    fn from(cfg: &WasmServerConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            plugin_dir: cfg.plugin_dir.clone(),
            default_memory_limit_bytes: cfg.default_memory_limit_bytes,
            default_timeout_ms: cfg.default_timeout_ms,
        }
    }
}

/// Sanitized compliance mode configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ComplianceSnapshot {
    /// Active compliance mode.
    pub mode: String,
    /// Whether synchronous audit writes are enabled.
    pub sync_audit_writes: bool,
    /// Whether audit records are immutable.
    pub immutable_audit: bool,
    /// Whether `SHA-256` hash chaining is enabled.
    pub hash_chain: bool,
}

impl From<&ComplianceServerConfig> for ComplianceSnapshot {
    fn from(cfg: &ComplianceServerConfig) -> Self {
        let resolved = cfg.to_compliance_config();
        Self {
            mode: resolved.mode.to_string(),
            sync_audit_writes: resolved.sync_audit_writes,
            immutable_audit: resolved.immutable_audit,
            hash_chain: resolved.hash_chain,
        }
    }
}

/// Sanitized provider configuration (headers/secrets removed).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderSnapshot {
    /// Provider name.
    pub name: String,
    /// Provider type.
    pub provider_type: String,
    /// Target URL (if configured).
    pub url: Option<String>,
    /// Number of custom headers configured (values hidden).
    pub header_count: usize,
    /// Whether a token is configured.
    pub has_token: bool,
    /// Whether an auth token is configured (Twilio).
    pub has_auth_token: bool,
    /// Whether a webhook URL is configured (Teams, Discord).
    pub has_webhook_url: bool,
    /// Email backend type (if configured).
    pub email_backend: Option<String>,
    /// AWS region (if configured).
    pub aws_region: Option<String>,
}

impl From<&ProviderConfig> for ProviderSnapshot {
    fn from(cfg: &ProviderConfig) -> Self {
        Self {
            name: cfg.name.clone(),
            provider_type: cfg.provider_type.clone(),
            url: cfg.url.clone(),
            header_count: cfg.headers.len(),
            has_token: cfg.token.is_some(),
            has_auth_token: cfg.auth_token.is_some(),
            has_webhook_url: cfg.webhook_url.is_some(),
            email_backend: cfg.email_backend.clone(),
            aws_region: cfg.aws_region.clone(),
        }
    }
}

/// Configuration for a pre-dispatch enrichment step, loaded from TOML.
///
/// # Example
///
/// ```toml
/// [[enrichments]]
/// name = "fetch-asg-state"
/// action_type = "set_desired_capacity"
/// provider = "aws-autoscaling"
/// lookup_provider = "aws-autoscaling"
/// resource_type = "auto_scaling_group"
/// merge_key = "current_asg_state"
/// timeout_seconds = 10
/// failure_policy = "fail_closed"
///
/// [enrichments.params]
/// auto_scaling_group_names = ["{{payload.asg_name}}"]
/// ```
#[derive(Debug, Deserialize)]
pub struct EnrichmentConfigToml {
    /// Human-readable name for this enrichment.
    pub name: String,
    /// Only apply to actions matching this namespace (if set).
    #[serde(default)]
    pub namespace: Option<String>,
    /// Only apply to actions matching this tenant (if set).
    #[serde(default)]
    pub tenant: Option<String>,
    /// Only apply to actions matching this action type (if set).
    #[serde(default)]
    pub action_type: Option<String>,
    /// Only apply to actions targeting this provider (if set).
    #[serde(default)]
    pub provider: Option<String>,
    /// The name of the resource lookup provider to use.
    pub lookup_provider: String,
    /// The resource type to look up (e.g., `"auto_scaling_group"`, `"instance"`).
    pub resource_type: String,
    /// Template for lookup parameters. Supports `{{payload.X}}`, `{{namespace}}`,
    /// `{{tenant}}`, `{{action_type}}` placeholders.
    pub params: serde_json::Value,
    /// Key under which to merge the lookup result into the action payload.
    pub merge_key: String,
    /// Timeout for the lookup call, in seconds (default: 5).
    #[serde(default = "default_enrichment_timeout")]
    pub timeout_seconds: u64,
    /// Failure policy: `"fail_open"` (default) or `"fail_closed"`.
    #[serde(default)]
    pub failure_policy: acteon_core::EnrichmentFailurePolicy,
}

fn default_enrichment_timeout() -> u64 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_defaults() {
        let config: TelemetryConfig = toml::from_str("").unwrap();
        assert!(!config.enabled);
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "acteon");
        assert!((config.sample_ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.protocol, "grpc");
        assert_eq!(config.timeout_seconds, 10);
        assert!(config.resource_attributes.is_empty());
    }

    #[test]
    fn telemetry_custom_config() {
        let toml = r#"
            enabled = true
            endpoint = "http://collector:4317"
            service_name = "my-acteon"
            sample_ratio = 0.5
            protocol = "http"
            timeout_seconds = 30

            [resource_attributes]
            "deployment.environment" = "staging"
            "host.name" = "node-1"
        "#;

        let config: TelemetryConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.endpoint, "http://collector:4317");
        assert_eq!(config.service_name, "my-acteon");
        assert!((config.sample_ratio - 0.5).abs() < f64::EPSILON);
        assert_eq!(config.protocol, "http");
        assert_eq!(config.timeout_seconds, 30);
        assert_eq!(config.resource_attributes.len(), 2);
        assert_eq!(
            config
                .resource_attributes
                .get("deployment.environment")
                .unwrap(),
            "staging"
        );
        assert_eq!(
            config.resource_attributes.get("host.name").unwrap(),
            "node-1"
        );
    }

    #[test]
    fn telemetry_disabled() {
        let toml = r#"
            enabled = false
        "#;

        let config: TelemetryConfig = toml::from_str(toml).unwrap();
        assert!(!config.enabled);
        // All other fields should still get defaults
        assert_eq!(config.endpoint, "http://localhost:4317");
        assert_eq!(config.service_name, "acteon");
    }

    #[test]
    fn telemetry_sample_ratio_bounds() {
        // Ratio = 0.0 (no sampling)
        let config: TelemetryConfig = toml::from_str("sample_ratio = 0.0").unwrap();
        assert!(config.sample_ratio <= 0.0);

        // Ratio = 0.5 (50% sampling)
        let config: TelemetryConfig = toml::from_str("sample_ratio = 0.5").unwrap();
        assert!((config.sample_ratio - 0.5).abs() < f64::EPSILON);

        // Ratio = 1.0 (100% sampling — default)
        let config: TelemetryConfig = toml::from_str("sample_ratio = 1.0").unwrap();
        assert!((config.sample_ratio - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn telemetry_protocol_grpc() {
        let config: TelemetryConfig = toml::from_str(r#"protocol = "grpc""#).unwrap();
        assert_eq!(config.protocol, "grpc");
    }

    #[test]
    fn telemetry_protocol_http() {
        let config: TelemetryConfig = toml::from_str(r#"protocol = "http""#).unwrap();
        assert_eq!(config.protocol, "http");
    }

    #[test]
    fn telemetry_empty_resource_attributes() {
        let config: TelemetryConfig = toml::from_str("[resource_attributes]").unwrap();
        assert!(config.resource_attributes.is_empty());
    }

    #[test]
    fn telemetry_in_acteon_config() {
        let toml = r#"
            [telemetry]
            enabled = true
            endpoint = "http://tempo:4317"
            sample_ratio = 0.1
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert!(config.telemetry.enabled);
        assert_eq!(config.telemetry.endpoint, "http://tempo:4317");
        assert!((config.telemetry.sample_ratio - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn telemetry_absent_from_acteon_config_uses_defaults() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        assert!(!config.telemetry.enabled);
        assert_eq!(config.telemetry.endpoint, "http://localhost:4317");
    }

    #[test]
    fn providers_default_empty() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        assert!(config.providers.is_empty());
    }

    #[test]
    fn providers_parsed_from_toml() {
        let toml = r#"
            [[providers]]
            name = "email"
            type = "webhook"
            url = "http://localhost:9999/webhook"

            [providers.headers]
            Authorization = "Bearer token"

            [[providers]]
            name = "slack"
            type = "log"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 2);

        assert_eq!(config.providers[0].name, "email");
        assert_eq!(config.providers[0].provider_type, "webhook");
        assert_eq!(
            config.providers[0].url.as_deref(),
            Some("http://localhost:9999/webhook")
        );
        assert_eq!(
            config.providers[0].headers.get("Authorization").unwrap(),
            "Bearer token"
        );

        assert_eq!(config.providers[1].name, "slack");
        assert_eq!(config.providers[1].provider_type, "log");
        assert!(config.providers[1].url.is_none());
        assert!(config.providers[1].headers.is_empty());
    }

    #[test]
    fn config_snapshot_masks_secrets() {
        let toml = r#"
            [server]
            host = "0.0.0.0"
            port = 9090
            approval_secret = "deadbeef"

            [[server.approval_keys]]
            id = "k1"
            secret = "cafebabe"

            [llm_guardrail]
            enabled = true
            api_key = "sk-secret-key-value"
            policy = "You are a safety checker for actions."

            [embedding]
            enabled = true
            api_key = "sk-embed-key"

            [[providers]]
            name = "email"
            type = "webhook"
            url = "http://localhost:9999/webhook"

            [providers.headers]
            Authorization = "Bearer secret-token"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let snapshot = ConfigSnapshot::from(&config);

        // Server: secrets not present in snapshot
        assert_eq!(snapshot.server.host, "0.0.0.0");
        assert_eq!(snapshot.server.port, 9090);

        // LLM guardrail: api_key masked as boolean
        assert!(snapshot.llm_guardrail.has_api_key);
        assert_eq!(
            snapshot.llm_guardrail.policy,
            "You are a safety checker for actions."
        );

        // UI: enabled and dist_path present
        assert!(snapshot.ui.enabled);
        assert_eq!(snapshot.ui.dist_path, "ui/dist");

        // Embedding: api_key masked as boolean
        assert!(snapshot.embedding.has_api_key);

        // Provider: headers hidden, only count shown
        assert_eq!(snapshot.providers.len(), 1);
        assert_eq!(snapshot.providers[0].name, "email");
        assert_eq!(snapshot.providers[0].header_count, 1);
    }

    #[test]
    fn config_snapshot_truncates_long_policy() {
        let long_policy = "x".repeat(200);
        let toml_str = format!(
            r#"
            [llm_guardrail]
            policy = "{long_policy}"
        "#
        );

        let config: ActeonConfig = toml::from_str(&toml_str).unwrap();
        let snapshot = ConfigSnapshot::from(&config);

        assert_eq!(snapshot.llm_guardrail.policy.len(), 103); // 100 chars + "..."
        assert!(snapshot.llm_guardrail.policy.ends_with("..."));
    }

    #[test]
    fn config_snapshot_empty_api_key_reports_false() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        let snapshot = ConfigSnapshot::from(&config);

        assert!(!snapshot.llm_guardrail.has_api_key);
        assert!(!snapshot.embedding.has_api_key);
    }

    #[test]
    fn twilio_provider_parsed_from_toml() {
        let toml = r#"
            [[providers]]
            name = "sms"
            type = "twilio"
            account_sid = "ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
            auth_token = "test-placeholder-token"
            from_number = "+15551234567"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "sms");
        assert_eq!(config.providers[0].provider_type, "twilio");
        assert_eq!(
            config.providers[0].account_sid.as_deref(),
            Some("ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        );
        assert_eq!(
            config.providers[0].auth_token.as_deref(),
            Some("test-placeholder-token")
        );
        assert_eq!(
            config.providers[0].from_number.as_deref(),
            Some("+15551234567")
        );

        let snapshot = ProviderSnapshot::from(&config.providers[0]);
        assert!(snapshot.has_auth_token);
        assert!(!snapshot.has_webhook_url);
        assert!(!snapshot.has_token);
    }

    #[test]
    fn teams_provider_parsed_from_toml() {
        let toml = r#"
            [[providers]]
            name = "teams-alerts"
            type = "teams"
            webhook_url = "https://outlook.office.com/webhook/test"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].provider_type, "teams");
        assert_eq!(
            config.providers[0].webhook_url.as_deref(),
            Some("https://outlook.office.com/webhook/test")
        );

        let snapshot = ProviderSnapshot::from(&config.providers[0]);
        assert!(snapshot.has_webhook_url);
        assert!(!snapshot.has_auth_token);
    }

    #[test]
    fn discord_provider_parsed_from_toml() {
        let toml = r#"
            [[providers]]
            name = "discord-alerts"
            type = "discord"
            webhook_url = "https://discord.com/api/webhooks/123/abc"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].provider_type, "discord");
        assert_eq!(
            config.providers[0].webhook_url.as_deref(),
            Some("https://discord.com/api/webhooks/123/abc")
        );

        let snapshot = ProviderSnapshot::from(&config.providers[0]);
        assert!(snapshot.has_webhook_url);
    }

    #[test]
    fn config_snapshot_serializes_to_json() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        let snapshot = ConfigSnapshot::from(&config);

        let json = serde_json::to_value(&snapshot).unwrap();
        assert!(json.is_object());
        assert!(json.get("server").is_some());
        assert!(json.get("llm_guardrail").is_some());
        assert!(json.get("providers").is_some());
    }

    #[test]
    fn background_config_defaults() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        assert!(!config.background.enable_recurring_actions);
        assert_eq!(config.background.recurring_check_interval_seconds, 60);
        assert!(!config.background.enable_scheduled_actions);
        assert_eq!(config.background.scheduled_check_interval_seconds, 5);
    }

    #[test]
    fn background_config_recurring_enabled() {
        let toml = r#"
            [background]
            enable_recurring_actions = true
            recurring_check_interval_seconds = 30
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert!(config.background.enable_recurring_actions);
        assert_eq!(config.background.recurring_check_interval_seconds, 30);
    }

    #[test]
    fn background_snapshot_includes_recurring_fields() {
        let toml = r#"
            [background]
            enable_recurring_actions = true
            recurring_check_interval_seconds = 120
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let snapshot = ConfigSnapshot::from(&config);
        assert!(snapshot.background.enable_recurring_actions);
        assert_eq!(snapshot.background.recurring_check_interval_seconds, 120);
    }

    #[test]
    fn background_snapshot_recurring_defaults() {
        let config: ActeonConfig = toml::from_str("").unwrap();
        let snapshot = ConfigSnapshot::from(&config);
        assert!(!snapshot.background.enable_recurring_actions);
        assert_eq!(snapshot.background.recurring_check_interval_seconds, 60);
    }

    #[test]
    fn config_snapshot_chains_summary() {
        let toml = r#"
            [chains]
            max_concurrent_advances = 8
            completed_chain_ttl_seconds = 3600

            [[chains.definitions]]
            name = "onboarding"
            timeout_seconds = 300

            [[chains.definitions.steps]]
            name = "step1"
            provider = "email"
            action_type = "send_welcome"
            payload_template = {}

            [[chains.definitions.steps]]
            name = "step2"
            provider = "slack"
            action_type = "notify"
            payload_template = {}
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let snapshot = ConfigSnapshot::from(&config);

        assert_eq!(snapshot.chains.max_concurrent_advances, 8);
        assert_eq!(snapshot.chains.completed_chain_ttl_seconds, 3600);
        assert_eq!(snapshot.chains.definitions.len(), 1);
        assert_eq!(snapshot.chains.definitions[0].name, "onboarding");
        assert_eq!(snapshot.chains.definitions[0].steps_count, 2);
        assert_eq!(snapshot.chains.definitions[0].timeout_seconds, Some(300));
    }

    #[test]
    fn wasm_config_defaults() {
        let config = WasmServerConfig::default();
        assert!(!config.enabled);
        assert!(config.plugin_dir.is_none());
        assert_eq!(config.default_memory_limit_bytes, 16 * 1024 * 1024);
        assert_eq!(config.default_timeout_ms, 100);
    }

    #[test]
    fn wasm_config_from_toml() {
        let toml = r#"
            [wasm]
            enabled = true
            plugin_dir = "/etc/acteon/plugins"
            default_memory_limit_bytes = 33554432
            default_timeout_ms = 200
        "#;
        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert!(config.wasm.enabled);
        assert_eq!(
            config.wasm.plugin_dir.as_deref(),
            Some("/etc/acteon/plugins")
        );
        assert_eq!(config.wasm.default_memory_limit_bytes, 33_554_432);
        assert_eq!(config.wasm.default_timeout_ms, 200);
    }

    #[test]
    fn wasm_config_omitted_uses_defaults() {
        let toml = "";
        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert!(!config.wasm.enabled);
        assert!(config.wasm.plugin_dir.is_none());
        assert_eq!(config.wasm.default_memory_limit_bytes, 16 * 1024 * 1024);
        assert_eq!(config.wasm.default_timeout_ms, 100);
    }

    #[test]
    fn wasm_snapshot_from_config() {
        let toml = r#"
            [wasm]
            enabled = true
            plugin_dir = "/plugins"
        "#;
        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let snapshot = ConfigSnapshot::from(&config);
        assert!(snapshot.wasm.enabled);
        assert_eq!(snapshot.wasm.plugin_dir.as_deref(), Some("/plugins"));
        assert_eq!(snapshot.wasm.default_memory_limit_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn chain_step_sub_chain_parsed_from_toml() {
        let toml = r#"
            [[chains.definitions]]
            name = "parent-chain"

            [[chains.definitions.steps]]
            name = "step1"
            provider = "email"
            action_type = "send_welcome"
            payload_template = {}

            [[chains.definitions.steps]]
            name = "invoke-notify"
            sub_chain = "notify-chain"

            [[chains.definitions.steps]]
            name = "step3"
            provider = "slack"
            action_type = "confirm"
            payload_template = {}
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.chains.definitions.len(), 1);
        let chain = &config.chains.definitions[0];
        assert_eq!(chain.steps.len(), 3);

        // Regular step
        assert_eq!(chain.steps[0].provider.as_deref(), Some("email"));
        assert_eq!(chain.steps[0].action_type.as_deref(), Some("send_welcome"));
        assert!(chain.steps[0].sub_chain.is_none());

        // Sub-chain step
        assert!(chain.steps[1].provider.is_none());
        assert!(chain.steps[1].action_type.is_none());
        assert_eq!(chain.steps[1].sub_chain.as_deref(), Some("notify-chain"));

        // Regular step after sub-chain
        assert_eq!(chain.steps[2].provider.as_deref(), Some("slack"));
        assert!(chain.steps[2].sub_chain.is_none());
    }

    #[test]
    fn chain_step_sub_chain_with_delay_and_on_failure() {
        let toml = r#"
            [[chains.definitions]]
            name = "with-options"

            [[chains.definitions.steps]]
            name = "invoke-child"
            sub_chain = "child-chain"
            delay_seconds = 30
            on_failure = "skip"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let step = &config.chains.definitions[0].steps[0];
        assert_eq!(step.sub_chain.as_deref(), Some("child-chain"));
        assert_eq!(step.delay_seconds, Some(30));
        assert_eq!(step.on_failure.as_deref(), Some("skip"));
    }

    #[test]
    fn chain_step_backward_compat_no_sub_chain() {
        // Existing TOML configs without sub_chain should still parse correctly.
        let toml = r#"
            [[chains.definitions]]
            name = "legacy"

            [[chains.definitions.steps]]
            name = "step1"
            provider = "email"
            action_type = "send"

            [chains.definitions.steps.payload_template]
            msg = "hello"
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let step = &config.chains.definitions[0].steps[0];
        assert_eq!(step.provider.as_deref(), Some("email"));
        assert_eq!(step.action_type.as_deref(), Some("send"));
        assert!(step.sub_chain.is_none());
    }

    #[test]
    fn enrichment_config_parses_with_defaults() {
        let toml = r#"
            [[enrichments]]
            name = "fetch-asg"
            lookup_provider = "cost-asg"
            resource_type = "auto_scaling_group"
            merge_key = "asg_data"

            [enrichments.params]
            auto_scaling_group_names = ["my-asg"]
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.enrichments.len(), 1);
        let e = &config.enrichments[0];
        assert_eq!(e.name, "fetch-asg");
        assert_eq!(e.lookup_provider, "cost-asg");
        assert_eq!(e.resource_type, "auto_scaling_group");
        assert_eq!(e.merge_key, "asg_data");
        assert_eq!(e.timeout_seconds, 5); // default
        assert_eq!(
            e.failure_policy,
            acteon_core::EnrichmentFailurePolicy::FailOpen
        ); // default
        assert!(e.namespace.is_none());
        assert!(e.tenant.is_none());
        assert!(e.action_type.is_none());
        assert!(e.provider.is_none());
    }

    #[test]
    fn enrichment_config_parses_full() {
        let toml = r#"
            [[enrichments]]
            name = "fetch-asg-state"
            namespace = "infra"
            tenant = "prod"
            action_type = "terminate_instances"
            provider = "cost-ec2"
            lookup_provider = "cost-asg"
            resource_type = "auto_scaling_group"
            merge_key = "current_asg_state"
            timeout_seconds = 10
            failure_policy = "fail_closed"

            [enrichments.params]
            auto_scaling_group_names = ["{{payload.asg_name}}"]
        "#;

        let config: ActeonConfig = toml::from_str(toml).unwrap();
        let e = &config.enrichments[0];
        assert_eq!(e.namespace.as_deref(), Some("infra"));
        assert_eq!(e.tenant.as_deref(), Some("prod"));
        assert_eq!(e.action_type.as_deref(), Some("terminate_instances"));
        assert_eq!(e.provider.as_deref(), Some("cost-ec2"));
        assert_eq!(e.timeout_seconds, 10);
        assert_eq!(
            e.failure_policy,
            acteon_core::EnrichmentFailurePolicy::FailClosed
        );
    }
}
