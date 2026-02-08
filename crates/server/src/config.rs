use std::collections::HashMap;

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
    /// Provider type: `"webhook"` or `"log"`.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Target URL (required for `"webhook"` type).
    pub url: Option<String>,
    /// Additional HTTP headers (used by `"webhook"` type).
    #[serde(default)]
    pub headers: HashMap<String, String>,
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
    /// Whether to process scheduled actions.
    #[serde(default)]
    pub enable_scheduled_actions: bool,
    /// How often to check for due scheduled actions (seconds).
    #[serde(default = "default_scheduled_check_interval")]
    pub scheduled_check_interval_seconds: u64,
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

fn default_scheduled_check_interval() -> u64 {
    5
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
    pub provider: String,
    /// Action type for the synthetic action.
    pub action_type: String,
    /// JSON payload template with `{{...}}` placeholders.
    pub payload_template: serde_json::Value,
    /// Per-step failure policy override: `"abort"`, `"skip"`, or `"dlq"`.
    pub on_failure: Option<String>,
    /// Optional delay in seconds before executing this step.
    pub delay_seconds: Option<u64>,
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
}
