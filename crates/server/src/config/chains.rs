use serde::Deserialize;

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
    /// Parallel step group that fans out to multiple sub-steps.
    /// Mutually exclusive with `provider` and `sub_chain`.
    #[serde(default)]
    pub parallel: Option<Box<ParallelStepGroupToml>>,
}

/// A parallel step group loaded from TOML.
#[derive(Debug, Deserialize)]
pub struct ParallelStepGroupToml {
    /// Sub-steps to execute concurrently.
    pub steps: Vec<ChainStepConfigToml>,
    /// Join policy: `"all"` (default) or `"any"`.
    #[serde(default)]
    pub join: Option<String>,
    /// Failure policy: `"fail_fast"` (default) or `"best_effort"`.
    #[serde(default)]
    pub on_failure: Option<String>,
    /// Optional timeout in seconds for the parallel group.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Optional maximum number of sub-steps executing concurrently.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
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
