use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Health check response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status indicator.
    #[schema(example = "ok")]
    pub status: String,
    /// Current gateway metrics snapshot.
    pub metrics: MetricsResponse,
}

/// Gateway dispatch metrics counters.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MetricsResponse {
    /// Total actions dispatched.
    #[schema(example = 142)]
    pub dispatched: u64,
    /// Actions successfully executed.
    #[schema(example = 130)]
    pub executed: u64,
    /// Actions deduplicated.
    #[schema(example = 5)]
    pub deduplicated: u64,
    /// Actions suppressed by rules.
    #[schema(example = 3)]
    pub suppressed: u64,
    /// Actions rerouted to another provider.
    #[schema(example = 2)]
    pub rerouted: u64,
    /// Actions throttled.
    #[schema(example = 1)]
    pub throttled: u64,
    /// Actions that failed after retries.
    #[schema(example = 1)]
    pub failed: u64,
    /// Actions allowed by the LLM guardrail.
    #[schema(example = 0)]
    pub llm_guardrail_allowed: u64,
    /// Actions denied by the LLM guardrail.
    #[schema(example = 0)]
    pub llm_guardrail_denied: u64,
    /// LLM guardrail evaluation errors.
    #[schema(example = 0)]
    pub llm_guardrail_errors: u64,
    /// Task chains started.
    #[schema(example = 0)]
    pub chains_started: u64,
    /// Task chains completed successfully.
    #[schema(example = 0)]
    pub chains_completed: u64,
    /// Task chains failed.
    #[schema(example = 0)]
    pub chains_failed: u64,
    /// Task chains cancelled.
    #[schema(example = 0)]
    pub chains_cancelled: u64,
    /// Actions pending human approval.
    #[schema(example = 0)]
    pub pending_approval: u64,
    /// Actions rejected because the provider circuit breaker was open.
    #[schema(example = 0)]
    pub circuit_open: u64,
    /// Circuit breaker state transitions (any direction).
    #[schema(example = 0)]
    pub circuit_transitions: u64,
    /// Actions rerouted to a fallback provider due to an open circuit.
    #[schema(example = 0)]
    pub circuit_fallbacks: u64,
    /// Actions scheduled for delayed execution.
    #[schema(example = 0)]
    pub scheduled: u64,
    /// Recurring actions dispatched successfully.
    #[schema(example = 0)]
    pub recurring_dispatched: u64,
    /// Recurring action dispatch errors.
    #[schema(example = 0)]
    pub recurring_errors: u64,
    /// Recurring actions skipped (disabled, expired, etc.).
    #[schema(example = 0)]
    pub recurring_skipped: u64,
    /// Actions blocked by tenant quota.
    #[schema(example = 0)]
    pub quota_exceeded: u64,
    /// Actions that passed with a quota warning.
    #[schema(example = 0)]
    pub quota_warned: u64,
    /// Actions degraded to a fallback provider due to quota.
    #[schema(example = 0)]
    pub quota_degraded: u64,
    /// Actions that triggered a quota notification to the tenant admin.
    #[schema(example = 0)]
    pub quota_notified: u64,
    /// State entries deleted by the retention reaper.
    #[schema(example = 0)]
    pub retention_deleted_state: u64,
    /// Retention reaper skipped entries due to compliance hold.
    #[schema(example = 0)]
    pub retention_skipped_compliance: u64,
    /// Retention reaper errors.
    #[schema(example = 0)]
    pub retention_errors: u64,
    /// Embedding cache metrics (present when embedding provider is enabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingMetricsResponse>,
}

/// Embedding cache and provider metrics.
///
/// Helps operators tune cache capacity and TTL settings. A low hit rate
/// suggests the cache is too small or the TTL too short. A high
/// `fail_open_count` indicates the embedding API is unreliable.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingMetricsResponse {
    /// Topic cache hits.
    #[schema(example = 500)]
    pub topic_cache_hits: u64,
    /// Topic cache misses (required provider API call).
    #[schema(example = 10)]
    pub topic_cache_misses: u64,
    /// Text cache hits.
    #[schema(example = 200)]
    pub text_cache_hits: u64,
    /// Text cache misses (required provider API call).
    #[schema(example = 50)]
    pub text_cache_misses: u64,
    /// Total embedding provider errors.
    #[schema(example = 0)]
    pub errors: u64,
    /// Times fail-open returned similarity `0.0` instead of an error.
    #[schema(example = 0)]
    pub fail_open_count: u64,
}

/// Summary of a loaded rule.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RuleSummary {
    /// Rule name.
    #[schema(example = "block-spam")]
    pub name: String,
    /// Evaluation priority (lower is evaluated first).
    #[schema(example = 10)]
    pub priority: i32,
    /// Whether the rule is currently enabled.
    #[schema(example = true)]
    pub enabled: bool,
    /// Optional human-readable description.
    #[schema(example = "Blocks spam actions")]
    pub description: Option<String>,
}

/// Request body for reloading rules from a directory.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReloadRequest {
    /// Path to the directory containing YAML rule files.
    #[schema(example = "/etc/acteon/rules")]
    pub directory: Option<String>,
}

/// Response after successfully reloading rules.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReloadResponse {
    /// Number of rules loaded.
    #[schema(example = 5)]
    pub reloaded: usize,
    /// Directory that was scanned.
    #[schema(example = "/etc/acteon/rules")]
    pub directory: String,
}

/// Request body for toggling a rule's enabled state.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SetEnabledRequest {
    /// Whether the rule should be enabled.
    #[schema(example = true)]
    pub enabled: bool,
}

/// Response after toggling a rule's enabled state.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SetEnabledResponse {
    /// Rule name.
    #[schema(example = "block-spam")]
    pub name: String,
    /// New enabled state.
    #[schema(example = true)]
    pub enabled: bool,
    /// Human-readable status string.
    #[schema(example = "enabled")]
    pub status: String,
}

/// Generic error response returned on failures.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error message.
    #[schema(example = "rule not found: unknown-rule")]
    pub error: String,
}
