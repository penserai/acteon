use serde::{Deserialize, Serialize};

/// Result of evaluating a single rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleTraceResult {
    /// The rule's condition evaluated to `true`.
    Matched,
    /// The rule's condition evaluated to `false`.
    NotMatched,
    /// The rule was skipped (disabled or a prior rule already matched).
    Skipped,
    /// An error occurred evaluating the rule's condition.
    Error,
}

impl RuleTraceResult {
    /// Return the `snake_case` string representation (matches serde serialization).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::NotMatched => "not_matched",
            Self::Skipped => "skipped",
            Self::Error => "error",
        }
    }
}

/// Trace entry for a single rule evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTraceEntry {
    /// Name of the rule.
    pub rule_name: String,
    /// Rule priority (lower = evaluated first).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Human-readable representation of the condition expression.
    pub condition_display: String,
    /// Result of evaluating this rule.
    pub result: RuleTraceResult,
    /// Time spent evaluating this rule in microseconds.
    pub evaluation_duration_us: u64,
    /// The action this rule would take (e.g. `"Deny"`, `"Suppress"`).
    pub action: String,
    /// Where the rule was loaded from (e.g. `"Inline"`, `"Yaml"`).
    pub source: String,
    /// Optional rule description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reason the rule was skipped, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Error message if evaluation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Contextual information captured during rule evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// The `time.*` map that was used during evaluation.
    pub time: serde_json::Value,
    /// Keys present in the environment (values omitted for security).
    pub environment_keys: Vec<String>,
    /// The effective timezone used for time-based conditions, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_timezone: Option<String>,
}

/// Complete trace of a rule evaluation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEvaluationTrace {
    /// The final verdict (e.g. `"allow"`, `"deny"`, `"suppress"`).
    pub verdict: String,
    /// Name of the first rule that matched, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    /// `true` when one or more rules produced an error during evaluation.
    #[serde(default)]
    pub has_errors: bool,
    /// Number of rules whose conditions were actually evaluated.
    pub total_rules_evaluated: usize,
    /// Number of rules that were skipped.
    pub total_rules_skipped: usize,
    /// Total wall-clock time for the entire evaluation in microseconds.
    pub evaluation_duration_us: u64,
    /// Per-rule trace entries in evaluation (priority) order.
    pub trace: Vec<RuleTraceEntry>,
    /// Contextual information about the evaluation environment.
    pub context: TraceContext,
    /// When the matched rule is a `Modify` action, contains the resulting
    /// payload after applying the JSON merge patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_payload: Option<serde_json::Value>,
}
