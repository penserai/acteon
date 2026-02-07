use std::path::Path;

use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction, RuleSource};
use acteon_rules::{RuleError, RuleFrontend};

use crate::parser::{
    YamlAction, YamlCondition, YamlFieldOp, YamlPredicate, YamlRule, YamlRuleFile,
};
use crate::template::parse_field_path;

/// A [`RuleFrontend`] implementation that parses YAML rule files and compiles
/// them into the Acteon expression IR.
pub struct YamlFrontend;

impl RuleFrontend for YamlFrontend {
    fn extensions(&self) -> &[&str] {
        &["yaml", "yml"]
    }

    fn parse(&self, content: &str) -> Result<Vec<Rule>, RuleError> {
        let file: YamlRuleFile = serde_yaml_ng::from_str(content)
            .map_err(|e| RuleError::Parse(format!("YAML parse error: {e}")))?;

        file.rules
            .into_iter()
            .map(|yaml_rule| compile_rule(yaml_rule, None))
            .collect()
    }

    fn parse_file(&self, path: &Path) -> Result<Vec<Rule>, RuleError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| RuleError::Parse(format!("cannot read {}: {e}", path.display())))?;

        let file: YamlRuleFile = serde_yaml_ng::from_str(&content).map_err(|e| {
            RuleError::Parse(format!("YAML parse error in {}: {e}", path.display()))
        })?;

        file.rules
            .into_iter()
            .map(|yaml_rule| compile_rule(yaml_rule, Some(path)))
            .collect()
    }
}

/// Compile a single `YamlRule` into the IR `Rule`.
fn compile_rule(yaml: YamlRule, file: Option<&Path>) -> Result<Rule, RuleError> {
    let condition = compile_condition(&yaml.condition)?;
    let action = compile_action(&yaml.action);
    let source = RuleSource::Yaml {
        file: file.map(|p| p.display().to_string()),
    };

    Ok(Rule {
        name: yaml.name,
        priority: yaml.priority,
        description: yaml.description,
        enabled: yaml.enabled,
        condition,
        action,
        source,
        version: 0,
        metadata: yaml.metadata,
    })
}

/// Compile a `YamlCondition` into an `Expr`.
fn compile_condition(cond: &YamlCondition) -> Result<Expr, RuleError> {
    match cond {
        YamlCondition::All { all } => {
            let exprs: Result<Vec<Expr>, RuleError> = all.iter().map(compile_predicate).collect();
            Ok(Expr::All(exprs?))
        }
        YamlCondition::Any { any } => {
            let exprs: Result<Vec<Expr>, RuleError> = any.iter().map(compile_predicate).collect();
            Ok(Expr::Any(exprs?))
        }
        YamlCondition::Single(pred) => compile_predicate(pred.as_ref()),
    }
}

/// Compile a `YamlPredicate` into an `Expr`.
fn compile_predicate(pred: &YamlPredicate) -> Result<Expr, RuleError> {
    match pred {
        YamlPredicate::FieldCheck { field, op } => {
            let field_expr = parse_field_path(field)?;
            compile_field_op(&field_expr, op)
        }
        YamlPredicate::StateSeen {
            state_seen,
            within_seconds,
        } => {
            let time_since = Expr::StateTimeSince(state_seen.clone());
            match within_seconds {
                Some(secs) => {
                    // state_time_since < within_seconds means the state was seen recently.
                    Ok(Expr::Binary(
                        BinaryOp::Lt,
                        Box::new(time_since),
                        Box::new(Expr::Int(i64::try_from(*secs).map_err(|e| {
                            RuleError::Parse(format!("within_seconds overflow: {e}"))
                        })?)),
                    ))
                }
                None => {
                    // Without a time bound, check that the time since is not i64::MAX
                    // (which means "never seen").
                    Ok(Expr::Binary(
                        BinaryOp::Lt,
                        Box::new(time_since),
                        Box::new(Expr::Int(i64::MAX)),
                    ))
                }
            }
        }
        YamlPredicate::StateCounter { state_counter, op } => {
            let counter_expr = Expr::StateCounter(state_counter.clone());
            compile_field_op(&counter_expr, op)
        }
        YamlPredicate::SemanticMatch {
            semantic_match,
            threshold,
            text_field,
        } => {
            let text_field_expr = text_field
                .as_ref()
                .map(|p| parse_field_path(p))
                .transpose()?
                .map(Box::new);
            Ok(Expr::SemanticMatch {
                topic: semantic_match.clone(),
                threshold: *threshold,
                text_field: text_field_expr,
            })
        }
        YamlPredicate::Nested(inner_cond) => compile_condition(inner_cond.as_ref()),
    }
}

/// Compile a `YamlFieldOp` into a comparison expression against the given `lhs`.
///
/// If multiple operator fields are set simultaneously, they are combined with
/// logical AND.
fn compile_field_op(lhs: &Expr, op: &YamlFieldOp) -> Result<Expr, RuleError> {
    let mut checks: Vec<Expr> = Vec::new();

    if let Some(ref val) = op.eq {
        checks.push(Expr::Binary(
            BinaryOp::Eq,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.ne {
        checks.push(Expr::Binary(
            BinaryOp::Ne,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.gt {
        checks.push(Expr::Binary(
            BinaryOp::Gt,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.lt {
        checks.push(Expr::Binary(
            BinaryOp::Lt,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.gte {
        checks.push(Expr::Binary(
            BinaryOp::Ge,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.lte {
        checks.push(Expr::Binary(
            BinaryOp::Le,
            Box::new(lhs.clone()),
            Box::new(json_to_expr(val)),
        ));
    }
    if let Some(ref val) = op.contains {
        checks.push(Expr::Binary(
            BinaryOp::Contains,
            Box::new(lhs.clone()),
            Box::new(Expr::String(val.clone())),
        ));
    }
    if let Some(ref val) = op.starts_with {
        checks.push(Expr::Binary(
            BinaryOp::StartsWith,
            Box::new(lhs.clone()),
            Box::new(Expr::String(val.clone())),
        ));
    }
    if let Some(ref val) = op.ends_with {
        checks.push(Expr::Binary(
            BinaryOp::EndsWith,
            Box::new(lhs.clone()),
            Box::new(Expr::String(val.clone())),
        ));
    }
    if let Some(ref val) = op.matches {
        checks.push(Expr::Binary(
            BinaryOp::Matches,
            Box::new(lhs.clone()),
            Box::new(Expr::String(val.clone())),
        ));
    }
    if let Some(ref vals) = op.in_list {
        let list_items: Vec<Expr> = vals.iter().map(json_to_expr).collect();
        checks.push(Expr::Binary(
            BinaryOp::In,
            Box::new(lhs.clone()),
            Box::new(Expr::List(list_items)),
        ));
    }

    match checks.len() {
        0 => Err(RuleError::Parse(
            "field operation has no comparison operator".to_owned(),
        )),
        1 => Ok(checks.into_iter().next().expect("length checked")),
        _ => Ok(Expr::All(checks)),
    }
}

/// Convert a `serde_json::Value` into the corresponding `Expr` literal.
fn json_to_expr(val: &serde_json::Value) -> Expr {
    match val {
        serde_json::Value::Null => Expr::Null,
        serde_json::Value::Bool(b) => Expr::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Expr::Int(i)
            } else if let Some(f) = n.as_f64() {
                Expr::Float(f)
            } else {
                Expr::Null
            }
        }
        serde_json::Value::String(s) => Expr::String(s.clone()),
        serde_json::Value::Array(arr) => Expr::List(arr.iter().map(json_to_expr).collect()),
        serde_json::Value::Object(obj) => Expr::Map(
            obj.iter()
                .map(|(k, v)| (k.clone(), json_to_expr(v)))
                .collect(),
        ),
    }
}

/// Compile a `YamlAction` into a `RuleAction`.
fn compile_action(action: &YamlAction) -> RuleAction {
    match action {
        YamlAction::Allow => RuleAction::Allow,
        YamlAction::Deny => RuleAction::Deny,
        YamlAction::Deduplicate { ttl_seconds } => RuleAction::Deduplicate {
            ttl_seconds: *ttl_seconds,
        },
        YamlAction::Suppress => RuleAction::Suppress,
        YamlAction::Reroute { target_provider } => RuleAction::Reroute {
            target_provider: target_provider.clone(),
        },
        YamlAction::Throttle {
            max_count,
            window_seconds,
        } => RuleAction::Throttle {
            max_count: *max_count,
            window_seconds: *window_seconds,
        },
        YamlAction::Modify { changes } => RuleAction::Modify {
            changes: changes.clone(),
        },
        YamlAction::StateMachine {
            state_machine,
            fingerprint_fields,
        } => RuleAction::StateMachine {
            state_machine: state_machine.clone(),
            fingerprint_fields: fingerprint_fields.clone(),
        },
        YamlAction::Group {
            group_by,
            group_wait_seconds,
            group_interval_seconds,
            max_group_size,
            template,
        } => RuleAction::Group {
            group_by: group_by.clone(),
            group_wait_seconds: *group_wait_seconds,
            group_interval_seconds: *group_interval_seconds,
            max_group_size: *max_group_size,
            template: template.clone(),
        },
        YamlAction::RequestApproval {
            notify_provider,
            timeout_seconds,
            message,
        } => RuleAction::RequestApproval {
            notify_provider: notify_provider.clone(),
            timeout_seconds: *timeout_seconds,
            message: message.clone(),
        },
        YamlAction::Chain { chain } => RuleAction::Chain {
            chain: chain.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use acteon_core::Action;
    use acteon_rules::engine::{EvalContext, RuleEngine, RuleVerdict};
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

    #[test]
    fn extensions_include_yaml_and_yml() {
        let fe = YamlFrontend;
        let exts = fe.extensions();
        assert!(exts.contains(&"yaml"));
        assert!(exts.contains(&"yml"));
    }

    #[test]
    fn parse_simple_yaml_rules() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
  - name: dedup-email
    priority: 2
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
        let rules = fe.parse(yaml).unwrap();
        assert_eq!(rules.len(), 2);

        assert_eq!(rules[0].name, "block-spam");
        assert_eq!(rules[0].priority, 1);
        assert!(rules[0].enabled);
        assert!(rules[0].action.is_suppress());

        assert_eq!(rules[1].name, "dedup-email");
        assert_eq!(rules[1].priority, 2);
        assert!(rules[1].action.is_deduplicate());
    }

    #[test]
    fn parse_invalid_yaml_produces_error() {
        let fe = YamlFrontend;
        let result = fe.parse("this is not valid yaml: [[[");
        assert!(result.is_err());
    }

    #[test]
    fn parse_yaml_missing_required_field() {
        let fe = YamlFrontend;
        let yaml = r"
rules:
  - priority: 1
    condition:
      field: action.action_type
      eq: spam
    action:
      type: suppress
";
        let result = fe.parse(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn compile_field_check_eq() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: eq-check
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        // The condition should be Binary(Eq, Field(Ident("action"), "action_type"), String("send_email"))
        match &rules[0].condition {
            Expr::Binary(BinaryOp::Eq, lhs, rhs) => {
                assert!(matches!(rhs.as_ref(), Expr::String(s) if s == "send_email"));
                assert!(matches!(lhs.as_ref(), Expr::Field(_, f) if f == "action_type"));
            }
            other => panic!("expected Binary(Eq, ...), got {other:?}"),
        }
    }

    #[test]
    fn compile_all_condition() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: all-check
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - field: action.payload.to
          contains: "@example.com"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(matches!(&rules[0].condition, Expr::All(exprs) if exprs.len() == 2));
    }

    #[test]
    fn compile_any_condition() {
        let fe = YamlFrontend;
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
        let rules = fe.parse(yaml).unwrap();
        assert!(matches!(&rules[0].condition, Expr::Any(exprs) if exprs.len() == 2));
    }

    #[test]
    fn compile_state_seen_with_window() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: recently-seen
    condition:
      state_seen: "last-email"
      within_seconds: 600
    action:
      type: suppress
"#;
        let rules = fe.parse(yaml).unwrap();
        // Should be: Binary(Lt, StateTimeSince("last-email"), Int(600))
        match &rules[0].condition {
            Expr::Binary(BinaryOp::Lt, lhs, rhs) => {
                assert!(matches!(lhs.as_ref(), Expr::StateTimeSince(k) if k == "last-email"));
                assert!(matches!(rhs.as_ref(), Expr::Int(600)));
            }
            other => panic!("expected Binary(Lt, StateTimeSince, Int), got {other:?}"),
        }
    }

    #[test]
    fn compile_state_seen_without_window() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: ever-seen
    condition:
      state_seen: "any-key"
    action:
      type: suppress
"#;
        let rules = fe.parse(yaml).unwrap();
        match &rules[0].condition {
            Expr::Binary(BinaryOp::Lt, lhs, rhs) => {
                assert!(matches!(lhs.as_ref(), Expr::StateTimeSince(k) if k == "any-key"));
                assert!(matches!(rhs.as_ref(), Expr::Int(i) if *i == i64::MAX));
            }
            other => panic!("expected Binary(Lt, ...), got {other:?}"),
        }
    }

    #[test]
    fn compile_state_counter() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: counter-check
    condition:
      state_counter: "email-count"
      gt: 100
    action:
      type: deny
"#;
        let rules = fe.parse(yaml).unwrap();
        match &rules[0].condition {
            Expr::Binary(BinaryOp::Gt, lhs, rhs) => {
                assert!(matches!(lhs.as_ref(), Expr::StateCounter(k) if k == "email-count"));
                assert!(matches!(rhs.as_ref(), Expr::Int(100)));
            }
            other => panic!("expected Binary(Gt, StateCounter, Int), got {other:?}"),
        }
    }

    #[test]
    fn compile_action_variants() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: r1
    condition:
      field: x
      eq: 1
    action:
      type: allow
  - name: r2
    condition:
      field: x
      eq: 1
    action:
      type: deny
  - name: r3
    condition:
      field: x
      eq: 1
    action:
      type: suppress
  - name: r4
    condition:
      field: x
      eq: 1
    action:
      type: deduplicate
      ttl_seconds: 120
  - name: r5
    condition:
      field: x
      eq: 1
    action:
      type: reroute
      target_provider: sms
  - name: r6
    condition:
      field: x
      eq: 1
    action:
      type: throttle
      max_count: 50
      window_seconds: 30
  - name: r7
    condition:
      field: x
      eq: 1
    action:
      type: modify
      changes:
        key: value
  - name: r8
    condition:
      field: x
      eq: 1
    action:
      type: state_machine
      state_machine: alert
      fingerprint_fields:
        - action_type
  - name: r9
    condition:
      field: x
      eq: 1
    action:
      type: group
      group_by:
        - metadata.cluster
  - name: r10
    condition:
      field: x
      eq: 1
    action:
      type: request_approval
      notify_provider: email
      timeout_seconds: 3600
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(rules[0].action.is_allow());
        assert!(rules[1].action.is_deny());
        assert!(rules[2].action.is_suppress());
        assert!(rules[3].action.is_deduplicate());
        assert!(rules[4].action.is_reroute());
        assert!(rules[5].action.is_throttle());
        assert!(rules[6].action.is_modify());
        assert!(rules[7].action.is_state_machine());
        assert!(rules[8].action.is_group());
        assert!(rules[9].action.is_request_approval());
    }

    #[test]
    fn rule_source_is_yaml() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: test
    condition:
      field: action.action_type
      eq: "test"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(matches!(&rules[0].source, RuleSource::Yaml { file: None }));
    }

    #[test]
    fn parse_file_nonexistent_path() {
        let fe = YamlFrontend;
        let result = fe.parse_file(Path::new("/nonexistent/rules.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn compile_contains_operator() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: contains-check
    condition:
      field: action.payload.to
      contains: "@example.com"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(matches!(
            &rules[0].condition,
            Expr::Binary(BinaryOp::Contains, _, _)
        ));
    }

    #[test]
    fn compile_starts_with_operator() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: starts-with-check
    condition:
      field: action.action_type
      starts_with: "send_"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(matches!(
            &rules[0].condition,
            Expr::Binary(BinaryOp::StartsWith, _, _)
        ));
    }

    #[test]
    fn compile_in_list_operator() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: in-list-check
    condition:
      field: action.action_type
      in_list: ["send_email", "send_sms"]
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        match &rules[0].condition {
            Expr::Binary(BinaryOp::In, _, rhs) => {
                assert!(matches!(rhs.as_ref(), Expr::List(items) if items.len() == 2));
            }
            other => panic!("expected Binary(In, ...), got {other:?}"),
        }
    }

    #[test]
    fn compile_multiple_ops_produces_all() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: range-check
    condition:
      field: action.payload.priority
      gt: 1
      lt: 10
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        // Multiple ops should produce an All([Gt, Lt])
        assert!(matches!(&rules[0].condition, Expr::All(exprs) if exprs.len() == 2));
    }

    // --- End-to-end engine tests ---

    #[tokio::test]
    async fn end_to_end_suppress_spam() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
  - name: allow-all
    priority: 100
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        // action_type is "send_email", not "spam", so block-spam does not fire.
        // allow-all matches.
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn end_to_end_dedup_email() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: dedup-email
    priority: 1
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
        let rules = fe.parse(yaml).unwrap();
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
    async fn end_to_end_no_match_allows() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: never-matches
    priority: 1
    condition:
      field: action.action_type
      eq: "nonexistent"
    action:
      type: deny
"#;
        let rules = fe.parse(yaml).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn end_to_end_priority_ordering() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: low-prio-allow
    priority: 100
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: allow
  - name: high-prio-suppress
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: suppress
"#;
        let rules = fe.parse(yaml).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        // high-prio-suppress (priority 1) should fire first.
        assert!(matches!(verdict, RuleVerdict::Suppress(_)));
    }

    #[tokio::test]
    async fn end_to_end_disabled_rule_skipped() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: disabled-deny
    priority: 1
    enabled: false
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: deny
"#;
        let rules = fe.parse(yaml).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn end_to_end_reroute() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: reroute-sms
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: reroute
      target_provider: "sms-fallback"
"#;
        let rules = fe.parse(yaml).unwrap();
        let engine = RuleEngine::new(rules);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = EvalContext::new(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Reroute {
                rule,
                target_provider,
            } => {
                assert_eq!(rule, "reroute-sms");
                assert_eq!(target_provider, "sms-fallback");
            }
            other => panic!("expected Reroute, got {other:?}"),
        }
    }

    #[test]
    fn parse_rule_with_metadata() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: guard-sql
    metadata:
      llm_policy: "Block any SQL containing DROP"
      owner: "security-team"
    condition:
      field: action.action_type
      eq: "sql_query"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].metadata.get("llm_policy").unwrap(),
            "Block any SQL containing DROP"
        );
        assert_eq!(rules[0].metadata.get("owner").unwrap(), "security-team");
    }

    #[test]
    fn parse_rule_without_metadata_defaults_empty() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: no-meta
    condition:
      field: action.action_type
      eq: "test"
    action:
      type: allow
"#;
        let rules = fe.parse(yaml).unwrap();
        assert!(rules[0].metadata.is_empty());
    }

    #[test]
    fn compile_semantic_match_predicate() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: route-infra
    condition:
      semantic_match: "Infrastructure issues"
      threshold: 0.75
      text_field: action.payload.message
    action:
      type: reroute
      target_provider: devops-pagerduty
"#;
        let rules = fe.parse(yaml).unwrap();
        assert_eq!(rules.len(), 1);
        match &rules[0].condition {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "Infrastructure issues");
                assert!((threshold - 0.75).abs() < f64::EPSILON);
                assert!(text_field.is_some());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    #[test]
    fn compile_semantic_match_no_text_field() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: route-billing
    condition:
      semantic_match: "Billing issues"
    action:
      type: reroute
      target_provider: billing-team
"#;
        let rules = fe.parse(yaml).unwrap();
        match &rules[0].condition {
            Expr::SemanticMatch {
                topic,
                threshold,
                text_field,
            } => {
                assert_eq!(topic, "Billing issues");
                assert!((threshold - 0.8).abs() < f64::EPSILON); // default
                assert!(text_field.is_none());
            }
            other => panic!("expected SemanticMatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn end_to_end_throttle() {
        let fe = YamlFrontend;
        let yaml = r#"
rules:
  - name: rate-limit
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: throttle
      max_count: 100
      window_seconds: 60
"#;
        let rules = fe.parse(yaml).unwrap();
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
}
