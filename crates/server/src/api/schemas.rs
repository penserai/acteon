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
