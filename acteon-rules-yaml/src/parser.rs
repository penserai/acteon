use serde::Deserialize;

/// Returns the default value `true` for serde.
const fn default_true() -> bool {
    true
}

/// Top-level YAML rule file containing a list of rules.
#[derive(Debug, Deserialize)]
pub struct YamlRuleFile {
    /// The list of rules defined in this file.
    pub rules: Vec<YamlRule>,
}

/// A single rule as represented in YAML.
#[derive(Debug, Deserialize)]
pub struct YamlRule {
    /// A human-readable name for the rule.
    pub name: String,
    /// Priority for ordering. Lower values are evaluated first.
    #[serde(default)]
    pub priority: i32,
    /// Optional description of what this rule does.
    pub description: Option<String>,
    /// Whether the rule is active. Defaults to `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The condition that must hold for the rule to fire.
    pub condition: YamlCondition,
    /// The action to take when the condition matches.
    pub action: YamlAction,
}

/// A condition expression that can combine multiple predicates.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum YamlCondition {
    /// All sub-predicates must be true (logical AND).
    All {
        /// The list of predicates that must all hold.
        all: Vec<YamlPredicate>,
    },
    /// Any sub-predicate must be true (logical OR).
    Any {
        /// The list of predicates where at least one must hold.
        any: Vec<YamlPredicate>,
    },
    /// A single predicate used directly as a condition.
    Single(Box<YamlPredicate>),
}

/// A single predicate within a condition.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum YamlPredicate {
    /// Check a field on the action against an operator.
    FieldCheck {
        /// Dot-separated field path (e.g. `action.payload.to`).
        field: String,
        /// The comparison operation to apply.
        #[serde(flatten)]
        op: YamlFieldOp,
    },
    /// Check whether a particular state key was seen within a time window.
    StateSeen {
        /// The state key to check.
        state_seen: String,
        /// Optional maximum age in seconds.
        within_seconds: Option<u64>,
    },
    /// Check a state counter against a comparison operator.
    StateCounter {
        /// The state counter key to check.
        state_counter: String,
        /// The comparison operation to apply.
        #[serde(flatten)]
        op: YamlFieldOp,
    },
    /// A nested condition (allows recursive `all` / `any` grouping).
    Nested(Box<YamlCondition>),
}

/// Describes which comparison operator to apply to a field or counter value.
///
/// Exactly one field should be set. If multiple are set, they are combined
/// with logical AND during compilation.
#[derive(Debug, Deserialize)]
pub struct YamlFieldOp {
    /// Equals comparison.
    pub eq: Option<serde_json::Value>,
    /// Not-equals comparison.
    pub ne: Option<serde_json::Value>,
    /// Greater-than comparison.
    pub gt: Option<serde_json::Value>,
    /// Less-than comparison.
    pub lt: Option<serde_json::Value>,
    /// Greater-than-or-equal comparison.
    pub gte: Option<serde_json::Value>,
    /// Less-than-or-equal comparison.
    pub lte: Option<serde_json::Value>,
    /// String contains check.
    pub contains: Option<String>,
    /// String starts-with check.
    pub starts_with: Option<String>,
    /// String ends-with check.
    pub ends_with: Option<String>,
    /// Regex match check.
    pub matches: Option<String>,
    /// Membership test against a list of values.
    pub in_list: Option<Vec<serde_json::Value>>,
}

/// The action to take when a rule fires.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum YamlAction {
    /// Allow the action to proceed.
    Allow,
    /// Deny the action.
    Deny,
    /// Deduplicate the action with an optional TTL.
    Deduplicate {
        /// Time-to-live in seconds for deduplication state.
        ttl_seconds: Option<u64>,
    },
    /// Suppress the action entirely.
    Suppress,
    /// Reroute the action to a different provider.
    Reroute {
        /// The target provider to route to.
        target_provider: String,
    },
    /// Throttle the action based on a sliding window.
    Throttle {
        /// Maximum number of actions allowed in the window.
        max_count: u64,
        /// Window size in seconds.
        window_seconds: u64,
    },
    /// Modify the action payload.
    Modify {
        /// JSON value describing the modifications.
        changes: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_rule_file() {
        let yaml = r"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: spam
    action:
      type: suppress
";
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 1);
        assert_eq!(file.rules[0].name, "block-spam");
        assert_eq!(file.rules[0].priority, 1);
        assert!(file.rules[0].enabled);
    }

    #[test]
    fn parse_all_condition() {
        let yaml = r#"
rules:
  - name: combined
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - field: action.payload.to
          contains: "@example.com"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 1);
        assert!(matches!(file.rules[0].condition, YamlCondition::All { .. }));
    }

    #[test]
    fn parse_any_condition() {
        let yaml = r#"
rules:
  - name: any-check
    condition:
      any:
        - field: action.action_type
          eq: "sms"
        - field: action.action_type
          eq: "email"
    action:
      type: allow
"#;
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(matches!(file.rules[0].condition, YamlCondition::Any { .. }));
    }

    #[test]
    fn parse_state_seen_predicate() {
        let yaml = r#"
rules:
  - name: state-check
    condition:
      state_seen: "last-email"
      within_seconds: 600
    action:
      type: suppress
"#;
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 1);
    }

    #[test]
    fn parse_state_counter_predicate() {
        let yaml = r#"
rules:
  - name: counter-check
    condition:
      state_counter: "email-count"
      gt: 100
    action:
      type: deny
"#;
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 1);
    }

    #[test]
    fn parse_all_action_types() {
        let yaml = r#"
rules:
  - name: a1
    condition:
      field: x
      eq: 1
    action:
      type: allow
  - name: a2
    condition:
      field: x
      eq: 1
    action:
      type: deny
  - name: a3
    condition:
      field: x
      eq: 1
    action:
      type: deduplicate
      ttl_seconds: 60
  - name: a4
    condition:
      field: x
      eq: 1
    action:
      type: suppress
  - name: a5
    condition:
      field: x
      eq: 1
    action:
      type: reroute
      target_provider: "sms-fallback"
  - name: a6
    condition:
      field: x
      eq: 1
    action:
      type: throttle
      max_count: 10
      window_seconds: 60
  - name: a7
    condition:
      field: x
      eq: 1
    action:
      type: modify
      changes:
        priority: high
"#;
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(file.rules.len(), 7);
    }

    #[test]
    fn defaults_enabled_true() {
        let yaml = r"
rules:
  - name: no-enabled-field
    condition:
      field: x
      eq: 1
    action:
      type: allow
";
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(file.rules[0].enabled);
    }

    #[test]
    fn explicit_disabled() {
        let yaml = r"
rules:
  - name: disabled-rule
    enabled: false
    condition:
      field: x
      eq: 1
    action:
      type: allow
";
        let file: YamlRuleFile = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!file.rules[0].enabled);
    }
}
