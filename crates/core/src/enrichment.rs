use serde::{Deserialize, Serialize};

/// Configuration for a pre-dispatch enrichment step.
///
/// Enrichments fetch external resource state and merge it into the action
/// payload before rule evaluation. This allows rules to check live data
/// (e.g., current `AutoScaling` group capacity) instead of relying solely
/// on static payload fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnrichmentConfig {
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
    /// Timeout for the lookup call, in seconds.
    #[serde(default = "default_enrichment_timeout_seconds")]
    pub timeout_seconds: u64,
    /// What to do if the lookup fails.
    #[serde(default)]
    pub failure_policy: EnrichmentFailurePolicy,
}

fn default_enrichment_timeout_seconds() -> u64 {
    5
}

/// Policy for handling enrichment lookup failures.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum EnrichmentFailurePolicy {
    /// Continue dispatch without the enrichment data (default).
    #[default]
    FailOpen,
    /// Reject the dispatch if enrichment fails.
    FailClosed,
}

/// Diagnostic outcome of an enrichment step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnrichmentOutcome {
    /// Name of the enrichment config that produced this outcome.
    pub name: String,
    /// The lookup provider used.
    pub provider: String,
    /// The resource type looked up.
    pub resource_type: String,
    /// Whether the lookup succeeded.
    pub success: bool,
    /// Error message if the lookup failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How long the lookup took in milliseconds.
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_config_serde_roundtrip() {
        let config = EnrichmentConfig {
            name: "fetch-asg-state".into(),
            namespace: Some("infra".into()),
            tenant: Some("tenant-1".into()),
            action_type: Some("set_desired_capacity".into()),
            provider: Some("aws-autoscaling".into()),
            lookup_provider: "aws-autoscaling".into(),
            resource_type: "auto_scaling_group".into(),
            params: serde_json::json!({
                "auto_scaling_group_names": ["{{payload.asg_name}}"]
            }),
            merge_key: "current_asg_state".into(),
            timeout_seconds: 10,
            failure_policy: EnrichmentFailurePolicy::FailClosed,
        };

        let json = serde_json::to_string(&config).unwrap();
        let back: EnrichmentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "fetch-asg-state");
        assert_eq!(back.namespace.as_deref(), Some("infra"));
        assert_eq!(back.timeout_seconds, 10);
        assert_eq!(back.failure_policy, EnrichmentFailurePolicy::FailClosed);
    }

    #[test]
    fn enrichment_config_deserializes_with_defaults() {
        let json = r#"{
            "name": "basic",
            "lookup_provider": "test",
            "resource_type": "widget",
            "params": {},
            "merge_key": "data"
        }"#;

        let config: EnrichmentConfig = serde_json::from_str(json).unwrap();
        assert!(config.namespace.is_none());
        assert!(config.tenant.is_none());
        assert!(config.action_type.is_none());
        assert!(config.provider.is_none());
        assert_eq!(config.timeout_seconds, 5);
        assert_eq!(config.failure_policy, EnrichmentFailurePolicy::FailOpen);
    }

    #[test]
    fn enrichment_failure_policy_serde() {
        let open: EnrichmentFailurePolicy = serde_json::from_str(r#""fail_open""#).unwrap();
        assert_eq!(open, EnrichmentFailurePolicy::FailOpen);

        let closed: EnrichmentFailurePolicy = serde_json::from_str(r#""fail_closed""#).unwrap();
        assert_eq!(closed, EnrichmentFailurePolicy::FailClosed);
    }

    #[test]
    fn enrichment_outcome_serde_roundtrip() {
        let outcome = EnrichmentOutcome {
            name: "fetch-asg".into(),
            provider: "aws-autoscaling".into(),
            resource_type: "auto_scaling_group".into(),
            success: true,
            error: None,
            duration_ms: 42,
        };

        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("error")); // skip_serializing_if = None
        let back: EnrichmentOutcome = serde_json::from_str(&json).unwrap();
        assert!(back.success);
        assert!(back.error.is_none());
        assert_eq!(back.duration_ms, 42);
    }

    #[test]
    fn enrichment_outcome_with_error() {
        let outcome = EnrichmentOutcome {
            name: "fetch-instance".into(),
            provider: "aws-ec2".into(),
            resource_type: "instance".into(),
            success: false,
            error: Some("connection refused".into()),
            duration_ms: 5000,
        };

        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("error"));
        let back: EnrichmentOutcome = serde_json::from_str(&json).unwrap();
        assert!(!back.success);
        assert_eq!(back.error.as_deref(), Some("connection refused"));
    }
}
