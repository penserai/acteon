use serde::Serialize;

use super::{
    ActeonConfig, AttachmentConfig, AuditConfig, AuditRedactConfig, AuthRefConfig,
    BackgroundProcessingConfig, ChainConfigToml, ChainsConfig, CircuitBreakerServerConfig,
    ComplianceServerConfig, EmbeddingServerConfig, EncryptionConfig, ExecutorConfig,
    LlmGuardrailServerConfig, ProviderConfig, RateLimitErrorBehavior, RateLimitRefConfig,
    RulesConfig, ServerConfig, StateConfig, TelemetryConfig, UiConfig, WasmServerConfig,
};

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
    /// Attachment configuration.
    pub attachments: AttachmentSnapshot,
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
            attachments: AttachmentSnapshot::from(&cfg.attachments),
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
    /// Whether template sync is enabled.
    pub enable_template_sync: bool,
    /// Template sync interval in seconds.
    pub template_sync_interval_seconds: u64,
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
            enable_template_sync: cfg.enable_template_sync,
            template_sync_interval_seconds: cfg.template_sync_interval_seconds,
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

/// Sanitized attachment configuration.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AttachmentSnapshot {
    /// Maximum decoded attachment size in bytes.
    pub max_inline_bytes: u64,
    /// Maximum attachments per action.
    pub max_attachments_per_action: usize,
}

impl From<&AttachmentConfig> for AttachmentSnapshot {
    fn from(cfg: &AttachmentConfig) -> Self {
        Self {
            max_inline_bytes: cfg.max_inline_bytes,
            max_attachments_per_action: cfg.max_attachments_per_action,
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
