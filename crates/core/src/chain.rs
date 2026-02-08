use std::collections::{HashMap, HashSet, VecDeque};

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

/// Comparison operator for branch conditions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchOperator {
    /// Equality check (`==`).
    Eq,
    /// Inequality check (`!=`).
    Neq,
    /// Check if a string field contains a substring.
    Contains,
    /// Check if a field exists and is not `null`.
    Exists,
}

/// A condition that determines which step to execute next after the current
/// step completes.
///
/// Conditions are evaluated in order; the first match wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchCondition {
    /// The field to evaluate. Supported paths:
    /// - `"success"` — boolean indicating step success/failure
    /// - `"body"` — the full response body
    /// - `"body.field.path"` — a nested field within the response body
    pub field: String,
    /// The comparison operator.
    pub operator: BranchOperator,
    /// The value to compare against. Ignored for the `exists` operator.
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// The name of the target step to jump to when this condition matches.
    pub target: String,
}

impl BranchCondition {
    /// Create a new branch condition.
    #[must_use]
    pub fn new(
        field: impl Into<String>,
        operator: BranchOperator,
        value: Option<serde_json::Value>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            operator,
            value,
            target: target.into(),
        }
    }

    /// Evaluate this condition against a step result.
    ///
    /// Returns `true` if the condition matches.
    #[must_use]
    pub fn evaluate(&self, step_result: &StepResult) -> bool {
        let field_value = self.resolve_field(step_result);
        match self.operator {
            BranchOperator::Eq => self.value.as_ref().is_some_and(|v| field_value == *v),
            BranchOperator::Neq => self.value.as_ref().is_some_and(|v| field_value != *v),
            BranchOperator::Contains => {
                if let (
                    serde_json::Value::String(haystack),
                    Some(serde_json::Value::String(needle)),
                ) = (&field_value, self.value.as_ref())
                {
                    haystack.contains(needle.as_str())
                } else {
                    false
                }
            }
            BranchOperator::Exists => !field_value.is_null(),
        }
    }

    /// Resolve the field path against a step result.
    fn resolve_field(&self, step_result: &StepResult) -> serde_json::Value {
        if self.field == "success" {
            return serde_json::Value::Bool(step_result.success);
        }

        if self.field == "body" {
            return step_result
                .response_body
                .clone()
                .unwrap_or(serde_json::Value::Null);
        }

        if let Some(path) = self.field.strip_prefix("body.") {
            let body = step_result
                .response_body
                .clone()
                .unwrap_or(serde_json::Value::Null);
            return resolve_json_path(&body, path);
        }

        serde_json::Value::Null
    }
}

/// Resolve a dotted path against a JSON value.
fn resolve_json_path(value: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = value.clone();
    for segment in path.split('.') {
        match current {
            serde_json::Value::Object(ref map) => {
                current = map.get(segment).cloned().unwrap_or(serde_json::Value::Null);
            }
            _ => return serde_json::Value::Null,
        }
    }
    current
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
    /// Optional list of branch conditions evaluated after this step completes.
    ///
    /// Conditions are evaluated in order; the first matching condition determines
    /// the next step. If no condition matches, `default_next` is used. If
    /// `default_next` is also `None`, the chain advances sequentially.
    #[serde(default)]
    pub branches: Vec<BranchCondition>,
    /// The default next step name when no branch condition matches.
    ///
    /// If `None` and no branch matches, the chain advances to the next
    /// sequential step.
    #[serde(default)]
    pub default_next: Option<String>,
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
            branches: Vec::new(),
            default_next: None,
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

    /// Add a branch condition to this step.
    #[must_use]
    pub fn with_branch(mut self, condition: BranchCondition) -> Self {
        self.branches.push(condition);
        self
    }

    /// Set the default next step when no branch condition matches.
    #[must_use]
    pub fn with_default_next(mut self, step_name: impl Into<String>) -> Self {
        self.default_next = Some(step_name.into());
        self
    }

    /// Returns `true` if this step has any branching configuration.
    #[must_use]
    pub fn has_branches(&self) -> bool {
        !self.branches.is_empty() || self.default_next.is_some()
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

    /// Build a map from step name to step index for quick lookups.
    #[must_use]
    pub fn step_index_map(&self) -> HashMap<String, usize> {
        self.steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), i))
            .collect()
    }

    /// Returns `true` if any step in this chain uses branching.
    #[must_use]
    pub fn has_branches(&self) -> bool {
        self.steps.iter().any(ChainStepConfig::has_branches)
    }

    /// Validate the chain configuration, checking for:
    /// - Duplicate step names
    /// - Branch targets referencing non-existent steps
    /// - Cycles in the branch graph (a step must not be reachable from itself)
    ///
    /// Returns a list of validation error messages. An empty list means valid.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let step_names: HashSet<&str> = self.steps.iter().map(|s| s.name.as_str()).collect();

        // Check for duplicate step names.
        if step_names.len() != self.steps.len() {
            let mut seen = HashSet::new();
            for step in &self.steps {
                if !seen.insert(&step.name) {
                    errors.push(format!("duplicate step name: `{}`", step.name));
                }
            }
        }

        // Check that all branch targets reference existing steps.
        for step in &self.steps {
            for branch in &step.branches {
                if !step_names.contains(branch.target.as_str()) {
                    errors.push(format!(
                        "step `{}` branches to non-existent step `{}`",
                        step.name, branch.target
                    ));
                }
            }
            if let Some(ref default_next) = step.default_next
                && !step_names.contains(default_next.as_str())
            {
                errors.push(format!(
                    "step `{}` has default_next targeting non-existent step `{default_next}`",
                    step.name
                ));
            }
        }

        // Check for cycles using BFS from each step.
        let index_map = self.step_index_map();
        for (i, step) in self.steps.iter().enumerate() {
            if !step.has_branches() {
                continue;
            }
            // Collect all possible next steps from this step (via branches).
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            for branch in &step.branches {
                if let Some(&target_idx) = index_map.get(&branch.target) {
                    queue.push_back(target_idx);
                }
            }
            if let Some(ref default_next) = step.default_next
                && let Some(&target_idx) = index_map.get(default_next)
            {
                queue.push_back(target_idx);
            }
            while let Some(idx) = queue.pop_front() {
                if idx == i {
                    errors.push(format!(
                        "cycle detected: step `{}` is reachable from itself via branches",
                        step.name
                    ));
                    break;
                }
                if !visited.insert(idx) {
                    continue;
                }
                if idx < self.steps.len() {
                    let target_step = &self.steps[idx];
                    for branch in &target_step.branches {
                        if let Some(&next_idx) = index_map.get(&branch.target) {
                            queue.push_back(next_idx);
                        }
                    }
                    if let Some(ref dn) = target_step.default_next
                        && let Some(&next_idx) = index_map.get(dn)
                    {
                        queue.push_back(next_idx);
                    }
                    // Also consider sequential fallthrough for steps without branches.
                    if !target_step.has_branches() && idx + 1 < self.steps.len() {
                        queue.push_back(idx + 1);
                    }
                }
            }
        }

        errors
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
    /// Ordered list of step names that have been executed (or are being
    /// executed), representing the actual execution path through the chain.
    /// For linear chains this matches the step order; for branching chains
    /// it records the branch path taken.
    #[serde(default)]
    pub execution_path: Vec<String>,
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

    #[test]
    fn branch_condition_evaluate_success_eq() {
        let cond = BranchCondition::new(
            "success",
            BranchOperator::Eq,
            Some(serde_json::Value::Bool(true)),
            "next-step",
        );
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: None,
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_body_field() {
        let cond = BranchCondition::new(
            "body.status",
            BranchOperator::Eq,
            Some(serde_json::json!("critical")),
            "escalate",
        );
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"status": "critical", "count": 5})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_neq() {
        let cond = BranchCondition::new(
            "body.level",
            BranchOperator::Neq,
            Some(serde_json::json!("info")),
            "alert",
        );
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"level": "error"})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_contains() {
        let cond = BranchCondition::new(
            "body.message",
            BranchOperator::Contains,
            Some(serde_json::json!("timeout")),
            "retry",
        );
        let result = StepResult {
            step_name: "call".into(),
            success: false,
            response_body: Some(serde_json::json!({"message": "connection timeout after 30s"})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_exists() {
        let cond = BranchCondition::new("body.data", BranchOperator::Exists, None, "process");
        let result = StepResult {
            step_name: "fetch".into(),
            success: true,
            response_body: Some(serde_json::json!({"data": [1, 2, 3]})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result));

        let result_no_data = StepResult {
            step_name: "fetch".into(),
            success: true,
            response_body: Some(serde_json::json!({"other": true})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_no_data));
    }

    #[test]
    fn branch_condition_missing_field_returns_null() {
        let cond = BranchCondition::new(
            "body.nonexistent.deep",
            BranchOperator::Eq,
            Some(serde_json::json!("x")),
            "step",
        );
        let result = StepResult {
            step_name: "s".into(),
            success: true,
            response_body: Some(serde_json::json!({"other": 1})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result));
    }

    #[test]
    fn chain_step_config_with_branches() {
        let step = ChainStepConfig::new("check", "api", "get_status", serde_json::json!({}))
            .with_branch(BranchCondition::new(
                "body.severity",
                BranchOperator::Eq,
                Some(serde_json::json!("high")),
                "escalate",
            ))
            .with_branch(BranchCondition::new(
                "body.severity",
                BranchOperator::Eq,
                Some(serde_json::json!("low")),
                "log-only",
            ))
            .with_default_next("notify");

        assert!(step.has_branches());
        assert_eq!(step.branches.len(), 2);
        assert_eq!(step.default_next.as_deref(), Some("notify"));
    }

    #[test]
    fn chain_step_config_without_branches_is_not_branching() {
        let step = ChainStepConfig::new("check", "api", "get_status", serde_json::json!({}));
        assert!(!step.has_branches());
    }

    #[test]
    fn chain_config_validate_valid_linear() {
        let config = ChainConfig::new("linear")
            .with_step(ChainStepConfig::new("a", "p", "t", serde_json::json!({})))
            .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})));
        assert!(config.validate().is_empty());
    }

    #[test]
    fn chain_config_validate_valid_branches() {
        let config = ChainConfig::new("branching")
            .with_step(
                ChainStepConfig::new("check", "api", "status", serde_json::json!({}))
                    .with_branch(BranchCondition::new(
                        "body.status",
                        BranchOperator::Eq,
                        Some(serde_json::json!("critical")),
                        "escalate",
                    ))
                    .with_default_next("log"),
            )
            .with_step(ChainStepConfig::new(
                "escalate",
                "pagerduty",
                "alert",
                serde_json::json!({}),
            ))
            .with_step(ChainStepConfig::new(
                "log",
                "logger",
                "info",
                serde_json::json!({}),
            ));
        assert!(config.validate().is_empty());
    }

    #[test]
    fn chain_config_validate_duplicate_names() {
        let config = ChainConfig::new("dupes")
            .with_step(ChainStepConfig::new("a", "p", "t", serde_json::json!({})))
            .with_step(ChainStepConfig::new("a", "p", "t", serde_json::json!({})));
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("duplicate step name"));
    }

    #[test]
    fn chain_config_validate_bad_branch_target() {
        let config = ChainConfig::new("bad-target")
            .with_step(
                ChainStepConfig::new("a", "p", "t", serde_json::json!({})).with_branch(
                    BranchCondition::new(
                        "success",
                        BranchOperator::Eq,
                        Some(serde_json::json!(true)),
                        "nonexistent",
                    ),
                ),
            )
            .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})));
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(errors[0].contains("non-existent step"));
    }

    #[test]
    fn chain_config_validate_cycle_detection() {
        // a -> b -> a (cycle)
        let config = ChainConfig::new("cycle")
            .with_step(
                ChainStepConfig::new("a", "p", "t", serde_json::json!({})).with_default_next("b"),
            )
            .with_step(
                ChainStepConfig::new("b", "p", "t", serde_json::json!({})).with_default_next("a"),
            );
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.contains("cycle detected")));
    }

    #[test]
    fn chain_config_step_index_map() {
        let config = ChainConfig::new("test")
            .with_step(ChainStepConfig::new(
                "alpha",
                "p",
                "t",
                serde_json::json!({}),
            ))
            .with_step(ChainStepConfig::new(
                "beta",
                "p",
                "t",
                serde_json::json!({}),
            ))
            .with_step(ChainStepConfig::new(
                "gamma",
                "p",
                "t",
                serde_json::json!({}),
            ));
        let map = config.step_index_map();
        assert_eq!(map.get("alpha"), Some(&0));
        assert_eq!(map.get("beta"), Some(&1));
        assert_eq!(map.get("gamma"), Some(&2));
    }

    #[test]
    fn branch_condition_serde_roundtrip() {
        let cond = BranchCondition::new(
            "body.status",
            BranchOperator::Eq,
            Some(serde_json::json!("critical")),
            "escalate",
        );
        let json = serde_json::to_string(&cond).unwrap();
        let back: BranchCondition = serde_json::from_str(&json).unwrap();
        assert_eq!(back.field, "body.status");
        assert_eq!(back.operator, BranchOperator::Eq);
        assert_eq!(back.target, "escalate");
    }

    #[test]
    fn chain_config_with_branches_serde_roundtrip() {
        let config = ChainConfig::new("branching")
            .with_step(
                ChainStepConfig::new("check", "api", "status", serde_json::json!({}))
                    .with_branch(BranchCondition::new(
                        "body.level",
                        BranchOperator::Eq,
                        Some(serde_json::json!("high")),
                        "escalate",
                    ))
                    .with_default_next("log"),
            )
            .with_step(ChainStepConfig::new(
                "escalate",
                "p",
                "t",
                serde_json::json!({}),
            ))
            .with_step(ChainStepConfig::new("log", "p", "t", serde_json::json!({})));

        let json = serde_json::to_string(&config).unwrap();
        let back: ChainConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steps[0].branches.len(), 1);
        assert_eq!(back.steps[0].default_next.as_deref(), Some("log"));
        assert!(back.has_branches());
    }

    #[test]
    fn backward_compatible_deserialization_no_branches() {
        // Simulate old JSON without branches or default_next fields.
        let json = r#"{
            "name": "old-chain",
            "steps": [{
                "name": "s1",
                "provider": "p",
                "action_type": "t",
                "payload_template": {}
            }]
        }"#;
        let config: ChainConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.steps[0].branches.len(), 0);
        assert!(config.steps[0].default_next.is_none());
        assert!(!config.has_branches());
    }

    #[test]
    fn chain_config_validate_valid_with_multiple_branch_points() {
        // Chain: check -> (branch to escalate or warn) -> notify
        //        escalate -> notify
        //        warn -> notify
        let config = ChainConfig::new("multi-branch-points")
            .with_step(
                ChainStepConfig::new("check", "api", "status", serde_json::json!({}))
                    .with_branch(BranchCondition::new(
                        "body.severity",
                        BranchOperator::Eq,
                        Some(serde_json::json!("high")),
                        "escalate",
                    ))
                    .with_branch(BranchCondition::new(
                        "body.severity",
                        BranchOperator::Eq,
                        Some(serde_json::json!("low")),
                        "warn",
                    ))
                    .with_default_next("notify"),
            )
            .with_step(
                ChainStepConfig::new("escalate", "pagerduty", "alert", serde_json::json!({}))
                    .with_default_next("notify"),
            )
            .with_step(
                ChainStepConfig::new("warn", "slack", "message", serde_json::json!({}))
                    .with_default_next("notify"),
            )
            .with_step(ChainStepConfig::new(
                "notify",
                "email",
                "send",
                serde_json::json!({}),
            ));
        assert!(
            config.validate().is_empty(),
            "valid chain with multiple branch points should have no errors"
        );
        assert!(config.has_branches());
    }

    #[test]
    fn chain_config_validate_default_next_to_nonexistent_step() {
        let config = ChainConfig::new("bad-default")
            .with_step(
                ChainStepConfig::new("a", "p", "t", serde_json::json!({}))
                    .with_default_next("ghost"),
            )
            .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})));
        let errors = config.validate();
        assert!(!errors.is_empty());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("default_next") && e.contains("ghost")),
            "should report default_next targeting non-existent step, got: {errors:?}"
        );
    }

    #[test]
    fn chain_config_validate_complex_cycle_a_b_c_a() {
        // A -> B -> C -> A (cycle through three steps)
        let config = ChainConfig::new("complex-cycle")
            .with_step(
                ChainStepConfig::new("a", "p", "t", serde_json::json!({})).with_default_next("b"),
            )
            .with_step(
                ChainStepConfig::new("b", "p", "t", serde_json::json!({})).with_default_next("c"),
            )
            .with_step(
                ChainStepConfig::new("c", "p", "t", serde_json::json!({})).with_default_next("a"),
            );
        let errors = config.validate();
        assert!(
            errors.iter().any(|e| e.contains("cycle detected")),
            "should detect A->B->C->A cycle, got: {errors:?}"
        );
    }

    #[test]
    fn backward_compatible_chain_state_deserialization() {
        // Simulate old ChainState JSON without execution_path.
        let json = serde_json::json!({
            "chain_id": "abc",
            "chain_name": "test",
            "origin_action": {
                "id": "id1",
                "namespace": "ns",
                "tenant": "t",
                "provider": "p",
                "action_type": "at",
                "payload": {},
                "created_at": "2026-01-01T00:00:00Z"
            },
            "current_step": 0,
            "total_steps": 1,
            "status": "running",
            "step_results": [null],
            "started_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "namespace": "ns",
            "tenant": "t"
        });
        let state: ChainState = serde_json::from_value(json).unwrap();
        assert!(state.execution_path.is_empty());
    }
}
