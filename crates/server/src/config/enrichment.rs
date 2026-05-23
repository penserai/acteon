use serde::Deserialize;

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
