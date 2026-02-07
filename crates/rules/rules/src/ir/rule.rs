use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::expr::Expr;

/// The action to take when a rule's condition matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleAction {
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
        /// JSON patch describing the modifications.
        changes: serde_json::Value,
    },
    /// A custom action for extension points.
    Custom {
        /// Name of the custom action handler.
        name: String,
        /// Parameters for the custom handler.
        params: serde_json::Value,
    },
    /// Process action through a state machine.
    StateMachine {
        /// Name of the state machine to use.
        state_machine: String,
        /// Fields to use for computing the fingerprint.
        fingerprint_fields: Vec<String>,
    },
    /// Group events for batched notification.
    Group {
        /// Fields to group events by.
        group_by: Vec<String>,
        /// Seconds to wait before sending first notification.
        group_wait_seconds: u64,
        /// Minimum seconds between notifications for same group.
        group_interval_seconds: u64,
        /// Maximum events in a single group.
        max_group_size: usize,
        /// Optional template name for group notification.
        template: Option<String>,
    },
    /// Request human approval before executing the action.
    RequestApproval {
        /// Provider to use for sending the approval notification.
        notify_provider: String,
        /// Timeout in seconds before the approval request expires.
        timeout_seconds: u64,
        /// Optional message to include in the approval notification.
        message: Option<String>,
    },
    /// Execute action as the first step of a named task chain.
    Chain {
        /// Name of the chain configuration to use.
        chain: String,
    },
}

/// Where a rule was loaded from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleSource {
    /// Loaded from a YAML file.
    Yaml {
        /// The file path, if available.
        file: Option<String>,
    },
    /// Created via API.
    Api,
    /// Defined inline in code.
    Inline,
}

/// A single rule combining a condition expression with an action.
///
/// Rules are evaluated in priority order (lower number = higher priority).
/// The first rule whose condition evaluates to `true` determines the verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// A human-readable name for the rule.
    pub name: String,
    /// Priority for ordering. Lower values are evaluated first.
    pub priority: i32,
    /// Optional description of what this rule does.
    pub description: Option<String>,
    /// Whether the rule is active.
    pub enabled: bool,
    /// The condition expression that must evaluate to `true` for the rule to fire.
    pub condition: Expr,
    /// The action to take when the condition matches.
    pub action: RuleAction,
    /// Where this rule was loaded from.
    pub source: RuleSource,
    /// Version number for tracking rule changes. Defaults to 0.
    #[serde(default)]
    pub version: u64,
    /// Arbitrary key-value metadata for the rule (e.g. `llm_policy` overrides).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional IANA timezone name for time-based conditions (e.g. `"US/Eastern"`).
    ///
    /// When set, `time.*` fields are evaluated in this timezone instead of UTC.
    /// Overrides the gateway-level `default_timezone`.
    #[serde(default)]
    pub timezone: Option<String>,
}

impl Rule {
    /// Create a new rule with the given name, condition, and action.
    ///
    /// Defaults to priority 0, enabled, and `Inline` source.
    pub fn new(name: impl Into<String>, condition: Expr, action: RuleAction) -> Self {
        Self {
            name: name.into(),
            priority: 0,
            description: None,
            enabled: true,
            condition,
            action,
            source: RuleSource::Inline,
            version: 0,
            metadata: HashMap::new(),
            timezone: None,
        }
    }

    /// Set the priority of this rule.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set the description of this rule.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the enabled state of this rule.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the source of this rule.
    #[must_use]
    pub fn with_source(mut self, source: RuleSource) -> Self {
        self.source = source;
        self
    }

    /// Set the version of this rule.
    #[must_use]
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Set the metadata for this rule.
    #[must_use]
    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the IANA timezone for time-based conditions (e.g. `"US/Eastern"`).
    #[must_use]
    pub fn with_timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = Some(tz.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ir::expr::BinaryOp;

    #[test]
    fn rule_construction() {
        let rule = Rule::new(
            "block-spam",
            Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "action_type".into(),
                )),
                Box::new(Expr::String("spam".into())),
            ),
            RuleAction::Deny,
        )
        .with_priority(10)
        .with_description("Block spam actions");

        assert_eq!(rule.name, "block-spam");
        assert_eq!(rule.priority, 10);
        assert_eq!(rule.description.as_deref(), Some("Block spam actions"));
        assert!(rule.enabled);
    }

    #[test]
    fn rule_serde_roundtrip() {
        let rule = Rule::new("test-rule", Expr::Bool(true), RuleAction::Allow).with_priority(5);

        let json = serde_json::to_string(&rule).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();

        assert_eq!(back.name, "test-rule");
        assert_eq!(back.priority, 5);
        assert!(back.enabled);
    }

    #[test]
    fn rule_action_variants_serde() {
        let actions: Vec<RuleAction> = vec![
            RuleAction::Allow,
            RuleAction::Deny,
            RuleAction::Deduplicate {
                ttl_seconds: Some(300),
            },
            RuleAction::Suppress,
            RuleAction::Reroute {
                target_provider: "fallback-sms".into(),
            },
            RuleAction::Throttle {
                max_count: 100,
                window_seconds: 60,
            },
            RuleAction::Modify {
                changes: serde_json::json!({"priority": "high"}),
            },
            RuleAction::Custom {
                name: "webhook".into(),
                params: serde_json::json!({"url": "https://example.com"}),
            },
            RuleAction::StateMachine {
                state_machine: "alert".into(),
                fingerprint_fields: vec!["action_type".into(), "metadata.cluster".into()],
            },
            RuleAction::Group {
                group_by: vec!["cluster".into(), "severity".into()],
                group_wait_seconds: 30,
                group_interval_seconds: 300,
                max_group_size: 100,
                template: Some("alert_group".into()),
            },
            RuleAction::RequestApproval {
                notify_provider: "email".into(),
                timeout_seconds: 86400,
                message: Some("Requires approval".into()),
            },
            RuleAction::Chain {
                chain: "search-summarize-email".into(),
            },
        ];

        for action in &actions {
            let json = serde_json::to_string(action).unwrap();
            let back: RuleAction = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{action:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn disabled_rule() {
        let rule = Rule::new("disabled", Expr::Bool(true), RuleAction::Allow).with_enabled(false);
        assert!(!rule.enabled);
    }

    #[test]
    fn rule_version_default_zero() {
        let rule = Rule::new("versioned", Expr::Bool(true), RuleAction::Allow);
        assert_eq!(rule.version, 0);
    }

    #[test]
    fn rule_with_version() {
        let rule = Rule::new("versioned", Expr::Bool(true), RuleAction::Allow).with_version(42);
        assert_eq!(rule.version, 42);
    }

    #[test]
    fn rule_version_serde_default() {
        // Serialize a rule, then strip the "version" key to simulate legacy JSON.
        let rule = Rule::new("test", Expr::Bool(true), RuleAction::Allow);
        let mut json_val: serde_json::Value = serde_json::to_value(&rule).unwrap();
        json_val.as_object_mut().unwrap().remove("version");
        let json_str = serde_json::to_string(&json_val).unwrap();

        // Deserializing without a "version" field should default to 0.
        let back: Rule = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back.version, 0);
    }

    #[test]
    fn rule_version_serde_roundtrip() {
        let rule = Rule::new("versioned", Expr::Bool(true), RuleAction::Allow).with_version(7);
        let json = serde_json::to_string(&rule).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, 7);
    }

    #[test]
    fn rule_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("llm_policy".into(), "Block DROP statements".into());
        let rule =
            Rule::new("guarded", Expr::Bool(true), RuleAction::Allow).with_metadata(meta.clone());
        assert_eq!(rule.metadata, meta);

        // Serde roundtrip preserves metadata.
        let json = serde_json::to_string(&rule).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.metadata.get("llm_policy").unwrap(),
            "Block DROP statements"
        );
    }

    #[test]
    fn rule_with_timezone() {
        let rule =
            Rule::new("tz-rule", Expr::Bool(true), RuleAction::Allow).with_timezone("US/Eastern");
        assert_eq!(rule.timezone.as_deref(), Some("US/Eastern"));
    }

    #[test]
    fn rule_timezone_serde_roundtrip() {
        let rule = Rule::new("tz-rule", Expr::Bool(true), RuleAction::Allow)
            .with_timezone("Europe/Berlin");
        let json = serde_json::to_string(&rule).unwrap();
        let back: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.timezone.as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn rule_timezone_serde_default() {
        let rule = Rule::new("test", Expr::Bool(true), RuleAction::Allow);
        let mut json_val: serde_json::Value = serde_json::to_value(&rule).unwrap();
        json_val.as_object_mut().unwrap().remove("timezone");
        let json_str = serde_json::to_string(&json_val).unwrap();

        let back: Rule = serde_json::from_str(&json_str).unwrap();
        assert!(back.timezone.is_none());
    }

    #[test]
    fn rule_metadata_serde_default() {
        // Deserializing without a "metadata" field should default to empty.
        let rule = Rule::new("test", Expr::Bool(true), RuleAction::Allow);
        let mut json_val: serde_json::Value = serde_json::to_value(&rule).unwrap();
        json_val.as_object_mut().unwrap().remove("metadata");
        let json_str = serde_json::to_string(&json_val).unwrap();

        let back: Rule = serde_json::from_str(&json_str).unwrap();
        assert!(back.metadata.is_empty());
    }
}
