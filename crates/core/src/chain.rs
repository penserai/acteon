use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Failure policy applied at the chain level when a step fails.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainFailurePolicy {
    /// Abort the chain and mark it as failed.
    #[default]
    Abort,
    /// Abort the chain without sending the failure to the DLQ.
    AbortNoDlq,
}

/// Per-step failure policy override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepFailurePolicy {
    /// Abort the chain on this step's failure.
    Abort,
    /// Skip this step and continue to the next.
    Skip,
    /// Push the failure to the DLQ and abort the chain.
    Dlq,
}

/// Configuration for a single step in a task chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStepConfig {
    /// Name of this step (used for `{{steps.NAME.*}}` references).
    pub name: String,
    /// Provider to execute this step with.
    pub provider: String,
    /// Action type for the synthetic action.
    pub action_type: String,
    /// Template for the step payload. Supports `{{origin.*}}`, `{{prev.*}}`,
    /// `{{steps.NAME.*}}`, `{{chain_id}}`, `{{step_index}}`.
    pub payload_template: serde_json::Value,
    /// Optional per-step failure policy override.
    #[serde(default)]
    pub on_failure: Option<StepFailurePolicy>,
    /// Optional delay in seconds before executing this step.
    ///
    /// When set, the chain will wait this many seconds before the step becomes
    /// eligible for execution. The delay is relative to when the previous step
    /// completes.
    #[serde(default)]
    pub delay_seconds: Option<u64>,
}

impl ChainStepConfig {
    /// Create a new step configuration.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        provider: impl Into<String>,
        action_type: impl Into<String>,
        payload_template: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            provider: provider.into(),
            action_type: action_type.into(),
            payload_template,
            on_failure: None,
            delay_seconds: None,
        }
    }

    /// Set the per-step failure policy.
    #[must_use]
    pub fn with_on_failure(mut self, policy: StepFailurePolicy) -> Self {
        self.on_failure = Some(policy);
        self
    }

    /// Set a delay (in seconds) before this step becomes eligible for execution.
    #[must_use]
    pub fn with_delay(mut self, seconds: u64) -> Self {
        self.delay_seconds = Some(seconds);
        self
    }
}

/// Target for outbound notifications dispatched when a chain is cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainNotificationTarget {
    /// Provider to dispatch the notification through.
    pub provider: String,
    /// Action type for the notification action.
    pub action_type: String,
}

/// Configuration for a task chain — a sequence of steps executed in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    /// Unique name for this chain (referenced from rules).
    pub name: String,
    /// Ordered list of steps to execute.
    pub steps: Vec<ChainStepConfig>,
    /// Chain-level failure policy.
    #[serde(default)]
    pub on_failure: ChainFailurePolicy,
    /// Optional timeout in seconds for the entire chain.
    pub timeout_seconds: Option<u64>,
    /// Optional notification target dispatched when the chain is cancelled.
    #[serde(default)]
    pub on_cancel: Option<ChainNotificationTarget>,
}

impl ChainConfig {
    /// Create a new chain configuration with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            steps: Vec::new(),
            on_failure: ChainFailurePolicy::default(),
            timeout_seconds: None,
            on_cancel: None,
        }
    }

    /// Add a step to the chain.
    #[must_use]
    pub fn with_step(mut self, step: ChainStepConfig) -> Self {
        self.steps.push(step);
        self
    }

    /// Set the chain-level failure policy.
    #[must_use]
    pub fn with_on_failure(mut self, policy: ChainFailurePolicy) -> Self {
        self.on_failure = policy;
        self
    }

    /// Set the chain timeout in seconds.
    #[must_use]
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    /// Set the notification target dispatched when the chain is cancelled.
    #[must_use]
    pub fn with_on_cancel(mut self, target: ChainNotificationTarget) -> Self {
        self.on_cancel = Some(target);
        self
    }
}

/// Status of a chain execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStatus {
    /// Chain is currently executing steps.
    Running,
    /// All steps completed successfully.
    Completed,
    /// A step failed and the chain was aborted.
    Failed,
    /// The chain was explicitly cancelled.
    Cancelled,
    /// The chain exceeded its timeout.
    TimedOut,
}

/// Result of a single chain step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Name of the step.
    pub step_name: String,
    /// Whether the step completed successfully.
    pub success: bool,
    /// Response body from the provider (if successful).
    pub response_body: Option<serde_json::Value>,
    /// Error message (if failed).
    pub error: Option<String>,
    /// When this step completed.
    pub completed_at: DateTime<Utc>,
}

/// Runtime state of a chain execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainState {
    /// Unique identifier for this chain execution.
    pub chain_id: String,
    /// Name of the chain configuration.
    pub chain_name: String,
    /// The original action that triggered the chain.
    pub origin_action: crate::Action,
    /// Index of the current step being executed (0-based).
    pub current_step: usize,
    /// Total number of steps in the chain.
    pub total_steps: usize,
    /// Current execution status.
    pub status: ChainStatus,
    /// Results for each completed step (indexed by step position).
    pub step_results: Vec<Option<StepResult>>,
    /// When the chain execution started.
    pub started_at: DateTime<Utc>,
    /// When the chain state was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the chain will time out (if a timeout is configured).
    pub expires_at: Option<DateTime<Utc>>,
    /// Namespace of the originating action.
    pub namespace: String,
    /// Tenant of the originating action.
    pub tenant: String,
    /// Reason for cancellation (if cancelled).
    #[serde(default)]
    pub cancel_reason: Option<String>,
    /// Who cancelled the chain (if cancelled).
    #[serde(default)]
    pub cancelled_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_config_builder() {
        let config = ChainConfig::new("my-chain")
            .with_step(ChainStepConfig::new(
                "step1",
                "provider-a",
                "do_thing",
                serde_json::json!({"key": "value"}),
            ))
            .with_step(
                ChainStepConfig::new("step2", "provider-b", "do_other", serde_json::json!({}))
                    .with_on_failure(StepFailurePolicy::Skip),
            )
            .with_on_failure(ChainFailurePolicy::AbortNoDlq)
            .with_timeout(300);

        assert_eq!(config.name, "my-chain");
        assert_eq!(config.steps.len(), 2);
        assert_eq!(config.steps[0].name, "step1");
        assert_eq!(config.steps[1].on_failure, Some(StepFailurePolicy::Skip));
        assert_eq!(config.on_failure, ChainFailurePolicy::AbortNoDlq);
        assert_eq!(config.timeout_seconds, Some(300));
    }

    #[test]
    fn default_failure_policy_is_abort() {
        assert_eq!(ChainFailurePolicy::default(), ChainFailurePolicy::Abort);
    }

    #[test]
    fn chain_config_serde_roundtrip() {
        let config = ChainConfig::new("test-chain")
            .with_step(ChainStepConfig::new(
                "search",
                "search-api",
                "web_search",
                serde_json::json!({"query": "{{origin.payload.query}}"}),
            ))
            .with_timeout(120);

        let json = serde_json::to_string(&config).unwrap();
        let back: ChainConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "test-chain");
        assert_eq!(back.steps.len(), 1);
        assert_eq!(back.timeout_seconds, Some(120));
    }

    #[test]
    fn chain_status_serde_roundtrip() {
        let statuses = vec![
            ChainStatus::Running,
            ChainStatus::Completed,
            ChainStatus::Failed,
            ChainStatus::Cancelled,
            ChainStatus::TimedOut,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let back: ChainStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, status);
        }
    }

    #[test]
    fn chain_failure_policy_serde_roundtrip() {
        let policies = vec![ChainFailurePolicy::Abort, ChainFailurePolicy::AbortNoDlq];
        for policy in &policies {
            let json = serde_json::to_string(policy).unwrap();
            let back: ChainFailurePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, policy);
        }
    }

    #[test]
    fn step_failure_policy_serde_roundtrip() {
        let policies = vec![
            StepFailurePolicy::Abort,
            StepFailurePolicy::Skip,
            StepFailurePolicy::Dlq,
        ];
        for policy in &policies {
            let json = serde_json::to_string(policy).unwrap();
            let back: StepFailurePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, policy);
        }
    }
}
