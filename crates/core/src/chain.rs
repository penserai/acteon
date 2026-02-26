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

/// Join policy for parallel step groups.
///
/// Determines when a parallel group is considered complete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelJoinPolicy {
    /// Wait for all sub-steps to complete.
    #[default]
    All,
    /// Return as soon as the first sub-step succeeds.
    Any,
}

/// Failure handling policy for parallel step groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelFailurePolicy {
    /// Cancel remaining sub-steps on the first failure.
    #[default]
    FailFast,
    /// Run all sub-steps and aggregate results, even if some fail.
    BestEffort,
}

/// Configuration for a group of steps that execute concurrently within a
/// single parent step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelStepGroup {
    /// Sub-steps to execute concurrently.
    pub steps: Vec<ChainStepConfig>,
    /// When the group is considered complete.
    #[serde(default)]
    pub join: ParallelJoinPolicy,
    /// How failures within the group are handled.
    #[serde(default)]
    pub on_failure: ParallelFailurePolicy,
    /// Optional timeout in seconds for the entire parallel group.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Optional maximum number of sub-steps executing concurrently.
    ///
    /// When set, sub-steps are dispatched in batches of this size using
    /// bounded concurrency. `None` (default) means all sub-steps run at once.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
}

/// Runtime tracking state for a parallel step group execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelExecutionState {
    /// Name of the parent parallel step.
    pub step_name: String,
    /// Index of the parent step in the chain.
    pub step_index: usize,
    /// Status of each sub-step, keyed by sub-step name.
    pub sub_steps: HashMap<String, ParallelSubStepStatus>,
    /// When the parallel group started executing.
    pub started_at: DateTime<Utc>,
    /// When the parallel group will time out.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Status of an individual sub-step within a parallel group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParallelSubStepStatus {
    /// Sub-step has not started yet.
    Pending,
    /// Sub-step is currently executing.
    Running,
    /// Sub-step completed successfully.
    Completed,
    /// Sub-step failed.
    Failed,
    /// Sub-step was cancelled (e.g., due to `fail_fast` or `any` join).
    Cancelled,
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
    /// Greater than (`>`). Supports numeric and lexicographic string comparison.
    Gt,
    /// Less than (`<`). Supports numeric and lexicographic string comparison.
    Lt,
    /// Greater than or equal (`>=`). Supports numeric and lexicographic string comparison.
    Gte,
    /// Less than or equal (`<=`). Supports numeric and lexicographic string comparison.
    Lte,
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
            BranchOperator::Gt => self.compare_ordered(&field_value, std::cmp::Ordering::is_gt),
            BranchOperator::Lt => self.compare_ordered(&field_value, std::cmp::Ordering::is_lt),
            BranchOperator::Gte => self.compare_ordered(&field_value, std::cmp::Ordering::is_ge),
            BranchOperator::Lte => self.compare_ordered(&field_value, std::cmp::Ordering::is_le),
        }
    }

    /// Compare the resolved field value against `self.value` using an ordered
    /// comparison and return whether the given predicate holds.
    ///
    /// - For `Number` JSON values: compare as `f64` via `partial_cmp` (NaN yields `false`).
    /// - For `String` JSON values: lexicographic comparison.
    /// - Type mismatch or missing comparison value: `false`.
    fn compare_ordered(
        &self,
        field_value: &serde_json::Value,
        predicate: impl Fn(std::cmp::Ordering) -> bool,
    ) -> bool {
        let Some(cmp_value) = self.value.as_ref() else {
            return false;
        };

        match (field_value, cmp_value) {
            (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                match (a.as_f64(), b.as_f64()) {
                    (Some(fa), Some(fb)) => fa.partial_cmp(&fb).is_some_and(&predicate),
                    _ => false,
                }
            }
            (serde_json::Value::String(a), serde_json::Value::String(b)) => predicate(a.cmp(b)),
            _ => false,
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
    /// Optional sub-chain name to invoke instead of dispatching to a provider.
    ///
    /// When set, this step invokes another chain by name. The parent chain
    /// pauses at this step until the sub-chain completes, then resumes with
    /// the sub-chain's result. Mutually exclusive with `provider`.
    #[serde(default)]
    pub sub_chain: Option<String>,
    /// Optional parallel step group that fans out to multiple sub-steps
    /// concurrently. Mutually exclusive with `provider` and `sub_chain`.
    ///
    /// Boxed to keep `ChainStepConfig` small on the stack (avoids stack
    /// overflows in debug builds with deeply nested async state machines).
    #[serde(default)]
    pub parallel: Option<Box<ParallelStepGroup>>,
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
            sub_chain: None,
            parallel: None,
        }
    }

    /// Create a new sub-chain step that invokes another chain by name.
    #[must_use]
    pub fn new_sub_chain(name: impl Into<String>, sub_chain_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            provider: String::new(),
            action_type: String::new(),
            payload_template: serde_json::Value::Object(serde_json::Map::new()),
            on_failure: None,
            delay_seconds: None,
            branches: Vec::new(),
            default_next: None,
            sub_chain: Some(sub_chain_name.into()),
            parallel: None,
        }
    }

    /// Create a new parallel step that fans out to multiple sub-steps.
    #[must_use]
    pub fn new_parallel(name: impl Into<String>, group: ParallelStepGroup) -> Self {
        Self {
            name: name.into(),
            provider: String::new(),
            action_type: String::new(),
            payload_template: serde_json::Value::Object(serde_json::Map::new()),
            on_failure: None,
            delay_seconds: None,
            branches: Vec::new(),
            default_next: None,
            sub_chain: None,
            parallel: Some(Box::new(group)),
        }
    }

    /// Returns `true` if this step invokes a sub-chain.
    #[must_use]
    pub fn is_sub_chain(&self) -> bool {
        self.sub_chain.is_some()
    }

    /// Returns `true` if this step is a parallel fan-out group.
    #[must_use]
    pub fn is_parallel(&self) -> bool {
        self.parallel.is_some()
    }

    /// Set the parallel step group.
    #[must_use]
    pub fn with_parallel(mut self, group: ParallelStepGroup) -> Self {
        self.parallel = Some(Box::new(group));
        self
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
    #[allow(clippy::too_many_lines)]
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

        // Check that sub-chain steps don't also specify a provider.
        for step in &self.steps {
            if step.sub_chain.is_some() && !step.provider.is_empty() {
                errors.push(format!(
                    "step `{}` has both `sub_chain` and `provider` set; they are mutually exclusive",
                    step.name
                ));
            }
        }

        // Check mutual exclusivity and validity of parallel steps.
        for step in &self.steps {
            if let Some(ref group) = step.parallel {
                if !step.provider.is_empty() {
                    errors.push(format!(
                        "step `{}` has both `parallel` and `provider` set; they are mutually exclusive",
                        step.name
                    ));
                }
                if step.sub_chain.is_some() {
                    errors.push(format!(
                        "step `{}` has both `parallel` and `sub_chain` set; they are mutually exclusive",
                        step.name
                    ));
                }
                if group.steps.is_empty() {
                    errors.push(format!(
                        "parallel step `{}` must have at least one sub-step",
                        step.name
                    ));
                }
                let mut sub_step_names = HashSet::new();
                for sub_step in &group.steps {
                    // Reject nested parallel.
                    if sub_step.parallel.is_some() {
                        errors.push(format!(
                            "nested parallel not allowed: sub-step `{}` in parallel step `{}`",
                            sub_step.name, step.name
                        ));
                    }
                    // Reject sub-chains inside parallel groups.
                    if sub_step.sub_chain.is_some() {
                        errors.push(format!(
                            "sub-chains not allowed inside parallel groups: sub-step `{}` in parallel step `{}`",
                            sub_step.name, step.name
                        ));
                    }
                    // Reject branches on individual sub-steps.
                    if sub_step.has_branches() {
                        errors.push(format!(
                            "branches not allowed on parallel sub-steps: sub-step `{}` in parallel step `{}`",
                            sub_step.name, step.name
                        ));
                    }
                    // Check for duplicate sub-step names within the group.
                    if !sub_step_names.insert(&sub_step.name) {
                        errors.push(format!(
                            "duplicate sub-step name `{}` in parallel step `{}`",
                            sub_step.name, step.name
                        ));
                    }
                    // Check that sub-step names don't conflict with top-level step names.
                    if step_names.contains(sub_step.name.as_str()) {
                        errors.push(format!(
                            "parallel sub-step `{}` conflicts with top-level step name in chain",
                            sub_step.name
                        ));
                    }
                }
                // Validate max_concurrency.
                if let Some(max) = group.max_concurrency
                    && max == 0
                {
                    errors.push(format!(
                        "parallel step `{}`: `max_concurrency` must be >= 1",
                        step.name
                    ));
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
    /// Chain is waiting for a sub-chain to complete.
    WaitingSubChain,
    /// Chain is executing a parallel step group.
    WaitingParallel,
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
    /// If this chain was spawned as a sub-chain, the parent chain's ID.
    #[serde(default)]
    pub parent_chain_id: Option<String>,
    /// If this chain was spawned as a sub-chain, the step index in the parent
    /// chain that triggered it.
    #[serde(default)]
    pub parent_step_index: Option<usize>,
    /// IDs of child chains spawned by sub-chain steps in this chain.
    #[serde(default)]
    pub child_chain_ids: Vec<String>,
    /// Runtime state of a currently-executing parallel step group.
    #[serde(default)]
    pub parallel_state: Option<ParallelExecutionState>,
    /// Results from sub-steps within a parallel group, keyed by sub-step name.
    /// Accessible via `{{steps.SUB_STEP_NAME.body.*}}` templates.
    #[serde(default)]
    pub parallel_sub_results: HashMap<String, StepResult>,
}

/// DFS coloring for cycle detection.
#[derive(Clone, Copy, PartialEq)]
enum DfsColor {
    White,
    Gray,
    Black,
}

/// DFS cycle detection helper for `validate_chain_graph`.
fn chain_graph_dfs<'a>(
    node: &'a str,
    adjacency: &HashMap<&str, Vec<&'a str>>,
    colors: &mut HashMap<&'a str, DfsColor>,
    path: &mut Vec<&'a str>,
    errors: &mut Vec<String>,
) {
    colors.insert(node, DfsColor::Gray);
    path.push(node);

    if let Some(neighbors) = adjacency.get(node) {
        for &neighbor in neighbors {
            match colors.get(neighbor) {
                Some(DfsColor::Gray) => {
                    // Found a cycle — build the cycle path for the error message.
                    let cycle_start = path.iter().position(|&n| n == neighbor).unwrap();
                    let cycle: Vec<&str> = path[cycle_start..].to_vec();
                    errors.push(format!(
                        "sub-chain cycle detected: {} -> {neighbor}",
                        cycle.join(" -> ")
                    ));
                }
                Some(DfsColor::White) => {
                    chain_graph_dfs(neighbor, adjacency, colors, path, errors);
                }
                _ => {} // Black — already fully explored
            }
        }
    }

    path.pop();
    colors.insert(node, DfsColor::Black);
}

/// Validate the sub-chain reference graph across all chain configurations.
///
/// Checks that:
/// - All `sub_chain` references point to known chain names
/// - There are no cycles in the sub-chain graph (A -> B -> C -> A)
///
/// Returns a list of validation error messages. An empty list means valid.
#[must_use]
pub fn validate_chain_graph<S: std::hash::BuildHasher>(
    chains: &HashMap<String, ChainConfig, S>,
) -> Vec<String> {
    let mut errors = Vec::new();

    // Build adjacency: chain_name -> set of sub-chain names it references.
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, config) in chains {
        let refs: Vec<&str> = config
            .steps
            .iter()
            .filter_map(|s| s.sub_chain.as_deref())
            .collect();

        // Check that all sub-chain references point to known chains.
        for sub in &refs {
            if !chains.contains_key(*sub) {
                errors.push(format!(
                    "chain `{name}` references unknown sub-chain `{sub}`"
                ));
            }
        }

        adjacency.insert(name.as_str(), refs);
    }

    let mut colors: HashMap<&str, DfsColor> = chains
        .keys()
        .map(|k| (k.as_str(), DfsColor::White))
        .collect();

    for &node in colors.clone().keys() {
        if colors[node] == DfsColor::White {
            let mut path = Vec::new();
            chain_graph_dfs(node, &adjacency, &mut colors, &mut path, &mut errors);
        }
    }

    errors
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
            ChainStatus::WaitingSubChain,
            ChainStatus::WaitingParallel,
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
        // Simulate old ChainState JSON without execution_path or sub-chain fields.
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
        assert!(state.parent_chain_id.is_none());
        assert!(state.parent_step_index.is_none());
        assert!(state.child_chain_ids.is_empty());
    }

    #[test]
    fn new_sub_chain_constructor() {
        let step = ChainStepConfig::new_sub_chain("invoke-notify", "notify-chain");
        assert_eq!(step.name, "invoke-notify");
        assert_eq!(step.sub_chain.as_deref(), Some("notify-chain"));
        assert!(step.provider.is_empty());
        assert!(step.action_type.is_empty());
        assert!(step.is_sub_chain());
    }

    #[test]
    fn regular_step_is_not_sub_chain() {
        let step = ChainStepConfig::new("do-thing", "provider-a", "action", serde_json::json!({}));
        assert!(!step.is_sub_chain());
    }

    #[test]
    fn sub_chain_step_with_branches() {
        let step = ChainStepConfig::new_sub_chain("invoke", "child-chain")
            .with_branch(BranchCondition::new(
                "success",
                BranchOperator::Eq,
                Some(serde_json::Value::Bool(true)),
                "next-step",
            ))
            .with_on_failure(StepFailurePolicy::Skip);
        assert!(step.is_sub_chain());
        assert!(step.has_branches());
        assert_eq!(step.on_failure, Some(StepFailurePolicy::Skip));
    }

    #[test]
    fn validate_rejects_sub_chain_with_provider() {
        let config = ChainConfig::new("bad").with_step({
            let mut step =
                ChainStepConfig::new("s1", "provider-a", "action", serde_json::json!({}));
            step.sub_chain = Some("other-chain".into());
            step
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("mutually exclusive")));
    }

    #[test]
    fn validate_chain_graph_no_cycles() {
        let mut chains = HashMap::new();
        chains.insert(
            "parent".into(),
            ChainConfig::new("parent")
                .with_step(ChainStepConfig::new_sub_chain("s1", "child"))
                .with_step(ChainStepConfig::new("s2", "p", "t", serde_json::json!({}))),
        );
        chains.insert(
            "child".into(),
            ChainConfig::new("child").with_step(ChainStepConfig::new(
                "s1",
                "p",
                "t",
                serde_json::json!({}),
            )),
        );
        assert!(validate_chain_graph(&chains).is_empty());
    }

    #[test]
    fn validate_chain_graph_direct_cycle() {
        let mut chains = HashMap::new();
        chains.insert(
            "a".into(),
            ChainConfig::new("a").with_step(ChainStepConfig::new_sub_chain("s1", "b")),
        );
        chains.insert(
            "b".into(),
            ChainConfig::new("b").with_step(ChainStepConfig::new_sub_chain("s1", "a")),
        );
        let errors = validate_chain_graph(&chains);
        assert!(errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn validate_chain_graph_transitive_cycle() {
        let mut chains = HashMap::new();
        chains.insert(
            "a".into(),
            ChainConfig::new("a").with_step(ChainStepConfig::new_sub_chain("s1", "b")),
        );
        chains.insert(
            "b".into(),
            ChainConfig::new("b").with_step(ChainStepConfig::new_sub_chain("s1", "c")),
        );
        chains.insert(
            "c".into(),
            ChainConfig::new("c").with_step(ChainStepConfig::new_sub_chain("s1", "a")),
        );
        let errors = validate_chain_graph(&chains);
        assert!(errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn validate_chain_graph_self_reference() {
        let mut chains = HashMap::new();
        chains.insert(
            "self-ref".into(),
            ChainConfig::new("self-ref")
                .with_step(ChainStepConfig::new_sub_chain("s1", "self-ref")),
        );
        let errors = validate_chain_graph(&chains);
        assert!(errors.iter().any(|e| e.contains("cycle")));
    }

    #[test]
    fn validate_chain_graph_unknown_reference() {
        let mut chains = HashMap::new();
        chains.insert(
            "parent".into(),
            ChainConfig::new("parent")
                .with_step(ChainStepConfig::new_sub_chain("s1", "nonexistent")),
        );
        let errors = validate_chain_graph(&chains);
        assert!(errors.iter().any(|e| e.contains("unknown sub-chain")));
    }

    #[test]
    fn waiting_sub_chain_status_serde() {
        let json = serde_json::to_string(&ChainStatus::WaitingSubChain).unwrap();
        assert_eq!(json, "\"waiting_sub_chain\"");
        let back: ChainStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ChainStatus::WaitingSubChain);
    }

    #[test]
    fn sub_chain_step_serde_roundtrip() {
        let step = ChainStepConfig::new_sub_chain("invoke-notify", "notify-chain").with_delay(10);
        let json = serde_json::to_string(&step).unwrap();
        let back: ChainStepConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sub_chain.as_deref(), Some("notify-chain"));
        assert!(back.is_sub_chain());
        assert!(back.provider.is_empty());
        assert_eq!(back.delay_seconds, Some(10));
    }

    #[test]
    fn backward_compatible_step_deserialization_no_sub_chain() {
        let json = r#"{
            "name": "old-step",
            "provider": "p",
            "action_type": "t",
            "payload_template": {}
        }"#;
        let step: ChainStepConfig = serde_json::from_str(json).unwrap();
        assert!(step.sub_chain.is_none());
        assert!(!step.is_sub_chain());
    }

    #[test]
    fn branch_condition_evaluate_gt_number() {
        let cond = BranchCondition::new(
            "body.count",
            BranchOperator::Gt,
            Some(serde_json::json!(5)),
            "high",
        );
        // count=10 > 5 → true
        let result_true = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_true));

        // count=3 > 5 → false
        let result_false = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 3})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_false));
    }

    #[test]
    fn branch_condition_evaluate_lt_number() {
        let cond = BranchCondition::new(
            "body.count",
            BranchOperator::Lt,
            Some(serde_json::json!(5)),
            "low",
        );
        // count=3 < 5 → true
        let result_true = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 3})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_true));

        // count=10 < 5 → false
        let result_false = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_false));
    }

    #[test]
    fn branch_condition_evaluate_gte_number() {
        let cond = BranchCondition::new(
            "body.count",
            BranchOperator::Gte,
            Some(serde_json::json!(5)),
            "at-least",
        );
        // count=10 >= 5 → true
        let result_above = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_above));

        // count=5 >= 5 → true (boundary)
        let result_equal = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 5})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_equal));

        // count=3 >= 5 → false
        let result_below = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 3})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_below));
    }

    #[test]
    fn branch_condition_evaluate_lte_number() {
        let cond = BranchCondition::new(
            "body.count",
            BranchOperator::Lte,
            Some(serde_json::json!(5)),
            "at-most",
        );
        // count=3 <= 5 → true
        let result_below = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 3})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_below));

        // count=5 <= 5 → true (boundary)
        let result_equal = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 5})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_equal));

        // count=10 <= 5 → false
        let result_above = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_above));
    }

    #[test]
    fn branch_condition_evaluate_gt_string() {
        let cond = BranchCondition::new(
            "body.grade",
            BranchOperator::Gt,
            Some(serde_json::json!("a")),
            "after-a",
        );
        // "b" > "a" lexicographically → true
        let result_true = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"grade": "b"})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(cond.evaluate(&result_true));

        // "a" > "a" → false (not strictly greater)
        let result_equal = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"grade": "a"})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result_equal));
    }

    #[test]
    fn branch_condition_evaluate_numeric_type_mismatch() {
        // Field value is a string, condition value is a number → false
        let cond = BranchCondition::new(
            "body.count",
            BranchOperator::Gt,
            Some(serde_json::json!(5)),
            "step",
        );
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": "not-a-number"})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_gt_missing_field() {
        let cond = BranchCondition::new(
            "body.nonexistent",
            BranchOperator::Gt,
            Some(serde_json::json!(5)),
            "step",
        );
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"other": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result));
    }

    #[test]
    fn branch_condition_evaluate_gt_no_value() {
        // Gt without a comparison value → false
        let cond = BranchCondition::new("body.count", BranchOperator::Gt, None, "step");
        let result = StepResult {
            step_name: "check".into(),
            success: true,
            response_body: Some(serde_json::json!({"count": 10})),
            error: None,
            completed_at: Utc::now(),
        };
        assert!(!cond.evaluate(&result));
    }

    #[test]
    fn branch_operator_serde_roundtrip_numeric() {
        let operators = vec![
            (BranchOperator::Eq, "\"eq\""),
            (BranchOperator::Neq, "\"neq\""),
            (BranchOperator::Contains, "\"contains\""),
            (BranchOperator::Exists, "\"exists\""),
            (BranchOperator::Gt, "\"gt\""),
            (BranchOperator::Lt, "\"lt\""),
            (BranchOperator::Gte, "\"gte\""),
            (BranchOperator::Lte, "\"lte\""),
        ];
        for (op, expected_json) in &operators {
            let json = serde_json::to_string(op).unwrap();
            assert_eq!(&json, expected_json, "serialization mismatch for {op:?}");
            let back: BranchOperator = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, op, "deserialization mismatch for {expected_json}");
        }
    }

    // -- Parallel step tests ------------------------------------------------

    #[test]
    fn parallel_join_policy_defaults() {
        assert_eq!(ParallelJoinPolicy::default(), ParallelJoinPolicy::All);
    }

    #[test]
    fn parallel_failure_policy_defaults() {
        assert_eq!(
            ParallelFailurePolicy::default(),
            ParallelFailurePolicy::FailFast
        );
    }

    #[test]
    fn parallel_step_constructor() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("notify_slack", "slack", "send", serde_json::json!({})),
                ChainStepConfig::new("notify_email", "email", "send", serde_json::json!({})),
            ],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: Some(30),
            max_concurrency: None,
        };
        let step = ChainStepConfig::new_parallel("fan-out", group);
        assert_eq!(step.name, "fan-out");
        assert!(step.is_parallel());
        assert!(!step.is_sub_chain());
        assert!(step.provider.is_empty());
        assert_eq!(step.parallel.as_ref().unwrap().steps.len(), 2);
        assert_eq!(step.parallel.as_ref().unwrap().timeout_seconds, Some(30));
    }

    #[test]
    fn parallel_step_is_parallel() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new("a", "p", "t", serde_json::json!({}))],
            join: ParallelJoinPolicy::Any,
            on_failure: ParallelFailurePolicy::BestEffort,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let step =
            ChainStepConfig::new("test", "p", "t", serde_json::json!({})).with_parallel(group);
        assert!(step.is_parallel());
    }

    #[test]
    fn parallel_step_serde_roundtrip() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("a", "p1", "t1", serde_json::json!({"key": "val"})),
                ChainStepConfig::new("b", "p2", "t2", serde_json::json!({})),
            ],
            join: ParallelJoinPolicy::Any,
            on_failure: ParallelFailurePolicy::BestEffort,
            timeout_seconds: Some(60),
            max_concurrency: None,
        };
        let step = ChainStepConfig::new_parallel("parallel-step", group);
        let json = serde_json::to_string(&step).unwrap();
        let back: ChainStepConfig = serde_json::from_str(&json).unwrap();
        assert!(back.is_parallel());
        let g = back.parallel.unwrap();
        assert_eq!(g.steps.len(), 2);
        assert_eq!(g.join, ParallelJoinPolicy::Any);
        assert_eq!(g.on_failure, ParallelFailurePolicy::BestEffort);
        assert_eq!(g.timeout_seconds, Some(60));
    }

    #[test]
    fn backward_compatible_step_deserialization_no_parallel() {
        let json = r#"{
            "name": "old-step",
            "provider": "p",
            "action_type": "t",
            "payload_template": {}
        }"#;
        let step: ChainStepConfig = serde_json::from_str(json).unwrap();
        assert!(!step.is_parallel());
        assert!(step.parallel.is_none());
    }

    #[test]
    fn validate_rejects_empty_parallel_group() {
        let group = ParallelStepGroup {
            steps: vec![],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step(ChainStepConfig::new_parallel("p", group));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("at least one sub-step")));
    }

    #[test]
    fn validate_rejects_nested_parallel() {
        let inner = ParallelStepGroup {
            steps: vec![ChainStepConfig::new(
                "inner",
                "p",
                "t",
                serde_json::json!({}),
            )],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let outer = ParallelStepGroup {
            steps: vec![ChainStepConfig::new_parallel("nested", inner)],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step(ChainStepConfig::new_parallel("p", outer));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("nested parallel")));
    }

    #[test]
    fn validate_rejects_sub_chain_in_parallel() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new_sub_chain("invoke", "other-chain")],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step(ChainStepConfig::new_parallel("p", group));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("sub-chains not allowed")));
    }

    #[test]
    fn validate_rejects_branches_on_parallel_sub_step() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("a", "p", "t", serde_json::json!({})).with_default_next("b"),
            ],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad")
            .with_step(ChainStepConfig::new_parallel("p", group))
            .with_step(ChainStepConfig::new("b", "p", "t", serde_json::json!({})));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("branches not allowed")));
    }

    #[test]
    fn validate_rejects_duplicate_sub_step_names() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("dup", "p1", "t1", serde_json::json!({})),
                ChainStepConfig::new("dup", "p2", "t2", serde_json::json!({})),
            ],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step(ChainStepConfig::new_parallel("p", group));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("duplicate sub-step name")));
    }

    #[test]
    fn validate_rejects_sub_step_name_conflict_with_chain_step() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new(
                "conflicting",
                "p1",
                "t1",
                serde_json::json!({}),
            )],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad")
            .with_step(ChainStepConfig::new_parallel("p", group))
            .with_step(ChainStepConfig::new(
                "conflicting",
                "p",
                "t",
                serde_json::json!({}),
            ));
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("conflicts with top-level"))
        );
    }

    #[test]
    fn validate_allows_branches_on_parent_parallel_step() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("notify_slack", "slack", "send", serde_json::json!({})),
                ChainStepConfig::new("notify_email", "email", "send", serde_json::json!({})),
            ],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("good")
            .with_step(
                ChainStepConfig::new_parallel("fan-out", group)
                    .with_branch(BranchCondition::new(
                        "success",
                        BranchOperator::Eq,
                        Some(serde_json::Value::Bool(true)),
                        "done",
                    ))
                    .with_default_next("fallback"),
            )
            .with_step(ChainStepConfig::new(
                "done",
                "p",
                "t",
                serde_json::json!({}),
            ))
            .with_step(ChainStepConfig::new(
                "fallback",
                "p",
                "t",
                serde_json::json!({}),
            ));
        assert!(
            config.validate().is_empty(),
            "parent parallel step with branches should be valid"
        );
    }

    #[test]
    fn parallel_execution_state_serde_roundtrip() {
        let state = ParallelExecutionState {
            step_name: "fan-out".into(),
            step_index: 1,
            sub_steps: HashMap::from([
                ("a".into(), ParallelSubStepStatus::Completed),
                ("b".into(), ParallelSubStepStatus::Running),
            ]),
            started_at: Utc::now(),
            expires_at: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: ParallelExecutionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.step_name, "fan-out");
        assert_eq!(back.sub_steps.len(), 2);
        assert_eq!(
            back.sub_steps.get("a"),
            Some(&ParallelSubStepStatus::Completed)
        );
    }

    #[test]
    fn chain_state_with_parallel_fields_serde_roundtrip() {
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
            "status": "waiting_parallel",
            "step_results": [null],
            "started_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "namespace": "ns",
            "tenant": "t",
            "parallel_sub_results": {
                "notify_slack": {
                    "step_name": "notify_slack",
                    "success": true,
                    "response_body": {"ok": true},
                    "error": null,
                    "completed_at": "2026-01-01T00:00:01Z"
                }
            }
        });
        let state: ChainState = serde_json::from_value(json).unwrap();
        assert_eq!(state.status, ChainStatus::WaitingParallel);
        assert_eq!(state.parallel_sub_results.len(), 1);
        assert!(
            state
                .parallel_sub_results
                .get("notify_slack")
                .unwrap()
                .success
        );
    }

    #[test]
    fn backward_compatible_chain_state_no_parallel_fields() {
        // Old JSON without parallel_state or parallel_sub_results.
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
        assert!(state.parallel_state.is_none());
        assert!(state.parallel_sub_results.is_empty());
    }

    #[test]
    fn waiting_parallel_status_serde() {
        let json = serde_json::to_string(&ChainStatus::WaitingParallel).unwrap();
        assert_eq!(json, "\"waiting_parallel\"");
        let back: ChainStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ChainStatus::WaitingParallel);
    }

    #[test]
    fn validate_rejects_parallel_with_provider() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new("a", "p", "t", serde_json::json!({}))],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step({
            let mut step =
                ChainStepConfig::new("s1", "provider-a", "action", serde_json::json!({}));
            step.parallel = Some(Box::new(group));
            step
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("parallel")
            && e.contains("provider")
            && e.contains("mutually exclusive")));
    }

    #[test]
    fn validate_rejects_parallel_with_sub_chain() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new("a", "p", "t", serde_json::json!({}))],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: None,
        };
        let config = ChainConfig::new("bad").with_step({
            let mut step = ChainStepConfig::new_sub_chain("s1", "other");
            step.parallel = Some(Box::new(group));
            step
        });
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("parallel")
            && e.contains("sub_chain")
            && e.contains("mutually exclusive")));
    }

    #[test]
    fn validate_rejects_zero_max_concurrency() {
        let group = ParallelStepGroup {
            steps: vec![ChainStepConfig::new("a", "p", "t", serde_json::json!({}))],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: Some(0),
        };
        let config = ChainConfig::new("bad").with_step(ChainStepConfig::new_parallel("p", group));
        let errors = config.validate();
        assert!(errors.iter().any(|e| e.contains("max_concurrency")));
    }

    #[test]
    fn validate_accepts_valid_max_concurrency() {
        let group = ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("a", "p1", "t1", serde_json::json!({})),
                ChainStepConfig::new("b", "p2", "t2", serde_json::json!({})),
            ],
            join: ParallelJoinPolicy::All,
            on_failure: ParallelFailurePolicy::FailFast,
            timeout_seconds: None,
            max_concurrency: Some(2),
        };
        let config = ChainConfig::new("good").with_step(ChainStepConfig::new_parallel("p", group));
        let errors = config.validate();
        assert!(errors.is_empty(), "expected no errors: {errors:?}");
    }
}
