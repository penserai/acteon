//! CEL rule frontend that parses YAML rule files with CEL condition expressions.

use std::path::Path;

use serde::Deserialize;

use acteon_rules::ir::rule::{Rule, RuleAction, RuleSource};
use acteon_rules::{RuleError, RuleFrontend};

use crate::parser::parse_cel_expr;

// ---------------------------------------------------------------------------
// Serde structures for the CEL rule file format
// ---------------------------------------------------------------------------

/// Returns the default value `true` for serde.
const fn default_true() -> bool {
    true
}

/// Top-level CEL rule file containing a list of rules.
#[derive(Debug, Deserialize)]
struct CelRuleFile {
    /// The list of rules defined in this file.
    rules: Vec<CelRule>,
}

/// A single rule as represented in the CEL rule file format.
///
/// The `condition` field is a CEL expression string instead of a structured
/// YAML condition.
#[derive(Debug, Deserialize)]
struct CelRule {
    /// A human-readable name for the rule.
    name: String,
    /// Priority for ordering. Lower values are evaluated first.
    #[serde(default)]
    priority: i32,
    /// Optional description of what this rule does.
    description: Option<String>,
    /// Whether the rule is active. Defaults to `true`.
    #[serde(default = "default_true")]
    enabled: bool,
    /// The CEL expression string for the condition.
    condition: String,
    /// The action to take when the condition matches.
    action: CelAction,
}

/// The action to take when a rule fires.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CelAction {
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
    /// Execute action through a task chain.
    Chain {
        /// Name of the chain configuration to use.
        chain: String,
    },
}

// ---------------------------------------------------------------------------
// CelFrontend implementation
// ---------------------------------------------------------------------------

/// A [`RuleFrontend`] implementation that parses YAML rule files where the
/// condition field is a CEL (Common Expression Language) expression string.
pub struct CelFrontend;

impl RuleFrontend for CelFrontend {
    fn extensions(&self) -> &[&str] {
        &["cel"]
    }

    fn parse(&self, content: &str) -> Result<Vec<Rule>, RuleError> {
        let file: CelRuleFile = serde_yaml_ng::from_str(content)
            .map_err(|e| RuleError::Parse(format!("CEL rule file parse error: {e}")))?;

        file.rules
            .into_iter()
            .map(|cel_rule| compile_rule(cel_rule, None))
            .collect()
    }

    fn parse_file(&self, path: &Path) -> Result<Vec<Rule>, RuleError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RuleError::Parse(format!("cannot read {}: {e}", path.display())))?;

        let file: CelRuleFile = serde_yaml_ng::from_str(&content).map_err(|e| {
            RuleError::Parse(format!(
                "CEL rule file parse error in {}: {e}",
                path.display()
            ))
        })?;

        file.rules
            .into_iter()
            .map(|cel_rule| compile_rule(cel_rule, Some(path)))
            .collect()
    }
}

/// Compile a single [`CelRule`] into the IR [`Rule`].
fn compile_rule(cel: CelRule, file: Option<&Path>) -> Result<Rule, RuleError> {
    let condition = parse_cel_expr(&cel.condition)?;
    let action = compile_action(&cel.action);
    let source = RuleSource::Yaml {
        file: file.map(|p| p.display().to_string()),
    };

    Ok(Rule {
        name: cel.name,
        priority: cel.priority,
        description: cel.description,
        enabled: cel.enabled,
        condition,
        action,
        source,
        version: 0,
        metadata: std::collections::HashMap::new(),
    })
}

/// Compile a [`CelAction`] into a [`RuleAction`].
fn compile_action(action: &CelAction) -> RuleAction {
    match action {
        CelAction::Allow => RuleAction::Allow,
        CelAction::Deny => RuleAction::Deny,
        CelAction::Deduplicate { ttl_seconds } => RuleAction::Deduplicate {
            ttl_seconds: *ttl_seconds,
        },
        CelAction::Suppress => RuleAction::Suppress,
        CelAction::Reroute { target_provider } => RuleAction::Reroute {
            target_provider: target_provider.clone(),
        },
        CelAction::Throttle {
            max_count,
            window_seconds,
        } => RuleAction::Throttle {
            max_count: *max_count,
            window_seconds: *window_seconds,
        },
        CelAction::Modify { changes } => RuleAction::Modify {
            changes: changes.clone(),
        },
        CelAction::Chain { chain } => RuleAction::Chain {
            chain: chain.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use acteon_core::Action;
    use acteon_rules::engine::{EvalContext, RuleEngine, RuleVerdict};
    use acteon_rules::ir::expr::{BinaryOp, Expr};
    use acteon_state_memory::MemoryStateStore;

    use super::*;

    fn test_action() -> Action {
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({
                "to": "user@example.com",
                "subject": "Hello",
                "priority": 5
            }),
        )
    }

    // --- Frontend basic tests ---

    #[test]
    fn extensions_include_cel() {
        let fe = CelFrontend;
        assert_eq!(fe.extensions(), &["cel"]);
    }

    #[test]
    fn parse_simple_cel_rule() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: "block-spam"
    priority: 1
    condition: 'action.action_type == "spam"'
    action:
      type: suppress
"#;
        let rules = fe.parse(content).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "block-spam");
        assert_eq!(rules[0].priority, 1);
        assert!(rules[0].enabled);
        assert!(rules[0].action.is_suppress());

        // Verify the condition is a Binary(Eq, Field, String)
        match &rules[0].condition {
            Expr::Binary(BinaryOp::Eq, lhs, rhs) => {
                assert!(matches!(lhs.as_ref(), Expr::Field(_, f) if f == "action_type"));
                assert!(matches!(rhs.as_ref(), Expr::String(s) if s == "spam"));
            }
            other => panic!("expected Binary(Eq, ...), got {other:?}"),
        }
    }

    #[test]
    fn parse_multiple_cel_rules() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: "block-spam"
    priority: 1
    condition: 'action.action_type == "spam"'
    action:
      type: suppress

  - name: "dedup-email"
    priority: 10
    condition: 'action.action_type == "send_email" && action.payload.to.contains("@")'
    action:
      type: deduplicate
      ttl_seconds: 300
"#;
        let rules = fe.parse(content).unwrap();
        assert_eq!(rules.len(), 2);

        assert_eq!(rules[0].name, "block-spam");
        assert!(rules[0].action.is_suppress());

        assert_eq!(rules[1].name, "dedup-email");
        assert!(rules[1].action.is_deduplicate());

        // Second rule condition should be And(Eq, Contains)
        match &rules[1].condition {
            Expr::Binary(BinaryOp::And, lhs, rhs) => {
                assert!(matches!(lhs.as_ref(), Expr::Binary(BinaryOp::Eq, _, _)));
                assert!(matches!(
                    rhs.as_ref(),
                    Expr::Binary(BinaryOp::Contains, _, _)
                ));
            }
            other => panic!("expected And(Eq, Contains), got {other:?}"),
        }
    }

    #[test]
    fn parse_all_action_types() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: r1
    condition: "true"
    action:
      type: allow
  - name: r2
    condition: "true"
    action:
      type: deny
  - name: r3
    condition: "true"
    action:
      type: suppress
  - name: r4
    condition: "true"
    action:
      type: deduplicate
      ttl_seconds: 120
  - name: r5
    condition: "true"
    action:
      type: reroute
      target_provider: sms
  - name: r6
    condition: "true"
    action:
      type: throttle
      max_count: 50
      window_seconds: 30
  - name: r7
    condition: "true"
    action:
      type: modify
      changes:
        key: value
"#;
        let rules = fe.parse(content).unwrap();
        assert_eq!(rules.len(), 7);
        assert!(rules[0].action.is_allow());
        assert!(rules[1].action.is_deny());
        assert!(rules[2].action.is_suppress());
        assert!(rules[3].action.is_deduplicate());
        assert!(rules[4].action.is_reroute());
        assert!(rules[5].action.is_throttle());
        assert!(rules[6].action.is_modify());
    }

    #[test]
    fn parse_disabled_rule() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: disabled
    enabled: false
    condition: "true"
    action:
      type: deny
"#;
        let rules = fe.parse(content).unwrap();
        assert!(!rules[0].enabled);
    }

    #[test]
    fn parse_rule_with_description() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: described
    description: "This rule has a description"
    condition: "true"
    action:
      type: allow
"#;
        let rules = fe.parse(content).unwrap();
        assert_eq!(
            rules[0].description.as_deref(),
            Some("This rule has a description")
        );
    }

    #[test]
    fn parse_invalid_yaml_produces_error() {
        let fe = CelFrontend;
        assert!(fe.parse("this is not valid yaml: [[[").is_err());
    }

    #[test]
    fn parse_invalid_cel_expression_produces_error() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: bad-cel
    condition: '1 + + + 2'
    action:
      type: allow
"#;
        assert!(fe.parse(content).is_err());
    }

    #[test]
    fn parse_file_nonexistent_path() {
        let fe = CelFrontend;
        let result = fe.parse_file(Path::new("/nonexistent/rules.cel"));
        assert!(result.is_err());
    }

    #[test]
    fn rule_source_is_yaml_no_file() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: test
    condition: "true"
    action:
      type: allow
"#;
        let rules = fe.parse(content).unwrap();
        assert!(matches!(&rules[0].source, RuleSource::Yaml { file: None }));
    }

    // --- End-to-end engine tests ---

    #[tokio::test]
    async fn e2e_suppress_spam() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: block-spam
    priority: 1
    condition: 'action.action_type == "spam"'
    action:
      type: suppress
  - name: allow-email
    priority: 100
    condition: 'action.action_type == "send_email"'
    action:
      type: allow
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        // action_type is "send_email", not "spam", so block-spam does not fire.
        // allow-email matches.
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn e2e_dedup_email() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: dedup-email
    priority: 1
    condition: 'action.action_type == "send_email" && action.payload.to.contains("@example.com")'
    action:
      type: deduplicate
      ttl_seconds: 300
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Deduplicate { ttl_seconds } => {
                assert_eq!(ttl_seconds, Some(300));
            }
            other => panic!("expected Deduplicate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn e2e_no_match_allows() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: never-matches
    priority: 1
    condition: 'action.action_type == "nonexistent"'
    action:
      type: deny
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn e2e_priority_ordering() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: low-prio-allow
    priority: 100
    condition: 'action.action_type == "send_email"'
    action:
      type: allow
  - name: high-prio-suppress
    priority: 1
    condition: 'action.action_type == "send_email"'
    action:
      type: suppress
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Suppress(_)));
    }

    #[tokio::test]
    async fn e2e_disabled_rule_skipped() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: disabled-deny
    priority: 1
    enabled: false
    condition: 'action.action_type == "send_email"'
    action:
      type: deny
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn e2e_complex_cel_condition() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: high-priority-email
    priority: 1
    condition: 'action.action_type == "send_email" && action.payload.priority > 3'
    action:
      type: reroute
      target_provider: "express-email"
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action(); // priority is 5
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Reroute {
                rule,
                target_provider,
            } => {
                assert_eq!(rule, "high-priority-email");
                assert_eq!(target_provider, "express-email");
            }
            other => panic!("expected Reroute, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn e2e_ternary_condition() {
        let fe = CelFrontend;
        // The ternary evaluates to true when action_type is "send_email"
        let content = r#"
rules:
  - name: ternary-test
    priority: 1
    condition: 'action.action_type == "send_email" ? true : false'
    action:
      type: suppress
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Suppress(_)));
    }

    #[tokio::test]
    async fn e2e_in_operator() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: in-check
    priority: 1
    condition: 'action.action_type in ["send_email", "send_sms"]'
    action:
      type: throttle
      max_count: 100
      window_seconds: 60
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Throttle {
                max_count,
                window_seconds,
                ..
            } => {
                assert_eq!(max_count, 100);
                assert_eq!(window_seconds, 60);
            }
            other => panic!("expected Throttle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn e2e_starts_with_method() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: starts-with-check
    priority: 1
    condition: 'action.action_type.startsWith("send")'
    action:
      type: allow
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn e2e_negation() {
        let fe = CelFrontend;
        let content = r#"
rules:
  - name: not-spam
    priority: 1
    condition: '!(action.action_type == "spam")'
    action:
      type: allow
"#;
        let rules = fe.parse(content).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action(); // action_type is "send_email", not "spam"
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }
}
