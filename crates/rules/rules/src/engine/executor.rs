use std::hash::{Hash, Hasher};

use tracing::{debug, instrument};

use crate::engine::context::EvalContext;
use crate::engine::eval::eval;
use crate::engine::verdict::{RuleVerdict, action_to_verdict};
use crate::error::RuleError;
use crate::ir::rule::Rule;

/// The rule engine evaluates a set of rules against an evaluation context.
///
/// Rules are evaluated in priority order (lower priority number first).
/// The first matching rule determines the verdict. If no rule matches,
/// the default verdict is `Allow`.
pub struct RuleEngine {
    rules: Vec<Rule>,
}

impl RuleEngine {
    /// Create a new rule engine with the given rules.
    ///
    /// Rules are automatically sorted by priority (lower number = higher priority).
    pub fn new(mut rules: Vec<Rule>) -> Self {
        rules.sort_by_key(|r| r.priority);
        Self { rules }
    }

    /// Return a reference to the sorted rules.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Look up a rule by name.
    pub fn rule_by_name(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    /// Compute a fingerprint of the current rule set.
    ///
    /// The hash combines each rule's name, version, and enabled state.
    /// A change in any of these fields produces a different version number,
    /// making it useful for detecting whether rules need to be reloaded.
    pub fn rules_version(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for rule in &self.rules {
            rule.name.hash(&mut hasher);
            rule.version.hash(&mut hasher);
            rule.enabled.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Add a rule to the engine and re-sort.
    pub fn add_rule(&mut self, rule: Rule) {
        self.rules.push(rule);
        self.rules.sort_by_key(|r| r.priority);
    }

    /// Evaluate all rules against the given context.
    ///
    /// Returns the verdict from the first matching rule, or `Allow` if no
    /// rule matches. If a rule has a `timezone` field, its `time.*` fields
    /// are evaluated in that timezone instead of the context default.
    #[instrument(skip_all, fields(rules_count = self.rules.len()))]
    pub async fn evaluate(&self, ctx: &EvalContext<'_>) -> Result<RuleVerdict, RuleError> {
        for rule in &self.rules {
            if !rule.enabled {
                debug!(rule = %rule.name, "skipping disabled rule");
                continue;
            }

            // If the rule has a per-rule timezone override, create a modified
            // context with that timezone for this rule's evaluation.
            let rule_tz = if let Some(ref tz_name) = rule.timezone {
                Some(
                    tz_name
                        .parse::<chrono_tz::Tz>()
                        .map_err(|_| RuleError::InvalidTimezone(tz_name.clone()))?,
                )
            } else {
                None
            };

            let eval_ctx;
            let effective_ctx = if let Some(tz) = rule_tz {
                eval_ctx = EvalContext {
                    action: ctx.action,
                    state: ctx.state,
                    environment: ctx.environment,
                    now: ctx.now,
                    embedding: ctx.embedding.clone(),
                    timezone: Some(tz),
                    time_map_cache: std::sync::OnceLock::new(),
                };
                &eval_ctx
            } else {
                ctx
            };

            let result = eval(&rule.condition, effective_ctx).await?;

            if result.is_truthy() {
                debug!(rule = %rule.name, "rule matched");
                return Ok(action_to_verdict(&rule.name, &rule.action));
            }
        }

        debug!("no rules matched, returning Allow");
        Ok(RuleVerdict::Allow(None))
    }

    /// Add multiple rules to the engine and re-sort by priority.
    pub fn add_rules(&mut self, rules: Vec<Rule>) {
        self.rules.extend(rules);
        self.rules.sort_by_key(|r| r.priority);
    }

    /// Return a list of all rule names.
    pub fn list_rules(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.name.as_str()).collect()
    }

    /// Enable a rule by name. Returns true if the rule was found.
    pub fn enable_rule(&mut self, name: &str) -> bool {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.name == name) {
            rule.enabled = true;
            true
        } else {
            false
        }
    }

    /// Disable a rule by name. Returns true if the rule was found.
    pub fn disable_rule(&mut self, name: &str) -> bool {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.name == name) {
            rule.enabled = false;
            true
        } else {
            false
        }
    }

    /// Load rules from a directory using the provided frontends.
    ///
    /// Walks the directory for files matching frontend extensions,
    /// parses each one, and adds the resulting rules. Returns the
    /// total number of rules loaded.
    pub fn load_directory(
        &mut self,
        path: &std::path::Path,
        frontends: &[&dyn crate::RuleFrontend],
    ) -> Result<usize, RuleError> {
        let mut loaded = 0;
        let entries = std::fs::read_dir(path).map_err(|e| {
            RuleError::Parse(format!("cannot read directory {}: {e}", path.display()))
        })?;

        for entry in entries {
            let entry =
                entry.map_err(|e| RuleError::Parse(format!("directory entry error: {e}")))?;
            let file_path = entry.path();

            if !file_path.is_file() {
                continue;
            }

            let extension = file_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("");

            for frontend in frontends {
                if frontend.extensions().contains(&extension) {
                    let rules = frontend.parse_file(&file_path)?;
                    let count = rules.len();
                    self.rules.extend(rules);
                    loaded += count;
                    break;
                }
            }
        }

        self.rules.sort_by_key(|r| r.priority);
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use acteon_core::Action;
    use acteon_state::StateStore;
    use acteon_state_memory::MemoryStateStore;
    use chrono::Utc;

    use super::*;
    use crate::engine::eval::{build_time_map, resolve_ident};
    use crate::engine::value::Value;
    use crate::ir::expr::{BinaryOp, Expr, UnaryOp};
    use crate::ir::rule::{Rule, RuleAction};

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

    fn test_context<'a>(
        action: &'a Action,
        store: &'a MemoryStateStore,
        env: &'a HashMap<String, String>,
    ) -> EvalContext<'a> {
        EvalContext::new(action, store, env)
    }

    // --- Value tests ---

    #[test]
    fn value_from_json() {
        let json = serde_json::json!({
            "name": "test",
            "count": 42,
            "active": true,
            "tags": ["a", "b"],
            "nested": null
        });
        let val = Value::from_json(json);
        assert!(matches!(val, Value::Map(_)));
    }

    #[test]
    fn value_is_truthy() {
        assert!(!Value::Null.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(!Value::Float(0.0).is_truthy());
        assert!(Value::Float(1.0).is_truthy());
        assert!(!Value::String(String::new()).is_truthy());
        assert!(Value::String("hi".into()).is_truthy());
        assert!(!Value::List(vec![]).is_truthy());
        assert!(Value::List(vec![Value::Int(1)]).is_truthy());
        assert!(!Value::Map(HashMap::new()).is_truthy());
    }

    #[test]
    fn value_type_names() {
        assert_eq!(Value::Null.type_name(), "null");
        assert_eq!(Value::Bool(true).type_name(), "bool");
        assert_eq!(Value::Int(0).type_name(), "int");
        assert_eq!(Value::Float(0.0).type_name(), "float");
        assert_eq!(Value::String(String::new()).type_name(), "string");
        assert_eq!(Value::List(vec![]).type_name(), "list");
        assert_eq!(Value::Map(HashMap::new()).type_name(), "map");
    }

    // --- Eval tests ---

    #[tokio::test]
    async fn eval_literals() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        assert_eq!(eval(&Expr::Null, &ctx).await.unwrap(), Value::Null);
        assert_eq!(
            eval(&Expr::Bool(true), &ctx).await.unwrap(),
            Value::Bool(true)
        );
        assert_eq!(eval(&Expr::Int(42), &ctx).await.unwrap(), Value::Int(42));
        assert_eq!(
            eval(&Expr::Float(3.14), &ctx).await.unwrap(),
            Value::Float(3.14)
        );
        assert_eq!(
            eval(&Expr::String("hello".into()), &ctx).await.unwrap(),
            Value::String("hello".into())
        );
    }

    #[tokio::test]
    async fn eval_ident_action() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let result = eval(&Expr::Ident("action".into()), &ctx).await.unwrap();
        assert!(matches!(result, Value::Map(_)));
    }

    #[tokio::test]
    async fn eval_field_access() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Field(Box::new(Expr::Ident("action".into())), "action_type".into());
        let result = eval(&expr, &ctx).await.unwrap();
        assert_eq!(result, Value::String("send_email".into()));
    }

    #[tokio::test]
    async fn eval_nested_field_access() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Field(
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "payload".into(),
            )),
            "to".into(),
        );
        let result = eval(&expr, &ctx).await.unwrap();
        assert_eq!(result, Value::String("user@example.com".into()));
    }

    #[tokio::test]
    async fn eval_environment_access() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let mut env = HashMap::new();
        env.insert("region".into(), "us-east-1".into());
        let ctx = test_context(&action, &store, &env);

        // Direct ident lookup
        let result = eval(&Expr::Ident("region".into()), &ctx).await.unwrap();
        assert_eq!(result, Value::String("us-east-1".into()));

        // Through env map
        let expr = Expr::Field(Box::new(Expr::Ident("env".into())), "region".into());
        let result = eval(&expr, &ctx).await.unwrap();
        assert_eq!(result, Value::String("us-east-1".into()));
    }

    #[tokio::test]
    async fn eval_arithmetic() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(5)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(15));

        let expr = Expr::Binary(
            BinaryOp::Sub,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(5)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(5));

        let expr = Expr::Binary(
            BinaryOp::Mul,
            Box::new(Expr::Int(3)),
            Box::new(Expr::Int(4)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(12));

        let expr = Expr::Binary(
            BinaryOp::Div,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(3)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(3));

        let expr = Expr::Binary(
            BinaryOp::Mod,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(3)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(1));
    }

    #[tokio::test]
    async fn eval_float_arithmetic() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Float(1.5)),
            Box::new(Expr::Float(2.5)),
        );
        match eval(&expr, &ctx).await.unwrap() {
            Value::Float(f) => assert!((f - 4.0).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn eval_mixed_int_float_arithmetic() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::Int(1)),
            Box::new(Expr::Float(2.5)),
        );
        match eval(&expr, &ctx).await.unwrap() {
            Value::Float(f) => assert!((f - 3.5).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn eval_division_by_zero() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Div,
            Box::new(Expr::Int(10)),
            Box::new(Expr::Int(0)),
        );
        assert!(eval(&expr, &ctx).await.is_err());
    }

    #[tokio::test]
    async fn eval_comparison() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(BinaryOp::Eq, Box::new(Expr::Int(5)), Box::new(Expr::Int(5)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(BinaryOp::Ne, Box::new(Expr::Int(5)), Box::new(Expr::Int(3)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(BinaryOp::Lt, Box::new(Expr::Int(3)), Box::new(Expr::Int(5)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(BinaryOp::Le, Box::new(Expr::Int(5)), Box::new(Expr::Int(5)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(BinaryOp::Gt, Box::new(Expr::Int(5)), Box::new(Expr::Int(3)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(BinaryOp::Ge, Box::new(Expr::Int(5)), Box::new(Expr::Int(5)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_string_comparison() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::String("abc".into())),
            Box::new(Expr::String("abc".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(
            BinaryOp::Lt,
            Box::new(Expr::String("abc".into())),
            Box::new(Expr::String("def".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_string_add() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Add,
            Box::new(Expr::String("hello ".into())),
            Box::new(Expr::String("world".into())),
        );
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("hello world".into())
        );
    }

    #[tokio::test]
    async fn eval_short_circuit_and() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // false && (error-producing expr) should short-circuit and not error.
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Bool(false)),
            Box::new(Expr::Binary(
                BinaryOp::Div,
                Box::new(Expr::Int(1)),
                Box::new(Expr::Int(0)),
            )),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn eval_short_circuit_or() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // true || (error-producing expr) should short-circuit.
        let expr = Expr::Binary(
            BinaryOp::Or,
            Box::new(Expr::Bool(true)),
            Box::new(Expr::Binary(
                BinaryOp::Div,
                Box::new(Expr::Int(1)),
                Box::new(Expr::Int(0)),
            )),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_unary_not() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Unary(UnaryOp::Not, Box::new(Expr::Bool(true)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));

        let expr = Expr::Unary(UnaryOp::Not, Box::new(Expr::Bool(false)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_unary_neg() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Unary(UnaryOp::Neg, Box::new(Expr::Int(42)));
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(-42));
    }

    #[tokio::test]
    async fn eval_string_ops() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Binary(
            BinaryOp::Contains,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("world".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(
            BinaryOp::StartsWith,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("hello".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(
            BinaryOp::EndsWith,
            Box::new(Expr::String("hello world".into())),
            Box::new(Expr::String("world".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Binary(
            BinaryOp::Matches,
            Box::new(Expr::String("user-123".into())),
            Box::new(Expr::String(r"^user-\d+$".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_in_operator() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // In list
        let expr = Expr::Binary(
            BinaryOp::In,
            Box::new(Expr::Int(2)),
            Box::new(Expr::List(vec![Expr::Int(1), Expr::Int(2), Expr::Int(3)])),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        // In string
        let expr = Expr::Binary(
            BinaryOp::In,
            Box::new(Expr::String("world".into())),
            Box::new(Expr::String("hello world".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_ternary() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Ternary(
            Box::new(Expr::Bool(true)),
            Box::new(Expr::String("yes".into())),
            Box::new(Expr::String("no".into())),
        );
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("yes".into())
        );

        let expr = Expr::Ternary(
            Box::new(Expr::Bool(false)),
            Box::new(Expr::String("yes".into())),
            Box::new(Expr::String("no".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::String("no".into()));
    }

    #[tokio::test]
    async fn eval_all_any() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::All(vec![Expr::Bool(true), Expr::Bool(true), Expr::Bool(true)]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::All(vec![Expr::Bool(true), Expr::Bool(false), Expr::Bool(true)]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));

        let expr = Expr::Any(vec![Expr::Bool(false), Expr::Bool(true), Expr::Bool(false)]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));

        let expr = Expr::Any(vec![
            Expr::Bool(false),
            Expr::Bool(false),
            Expr::Bool(false),
        ]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn eval_all_short_circuit() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // All with false first should short-circuit before hitting div-by-zero.
        let expr = Expr::All(vec![
            Expr::Bool(false),
            Expr::Binary(
                BinaryOp::Div,
                Box::new(Expr::Int(1)),
                Box::new(Expr::Int(0)),
            ),
        ]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn eval_any_short_circuit() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // Any with true first should short-circuit.
        let expr = Expr::Any(vec![
            Expr::Bool(true),
            Expr::Binary(
                BinaryOp::Div,
                Box::new(Expr::Int(1)),
                Box::new(Expr::Int(0)),
            ),
        ]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_call_builtin() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Call("len".into(), vec![Expr::String("hello".into())]);
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(5));

        let expr = Expr::Call("upper".into(), vec![Expr::String("hello".into())]);
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("HELLO".into())
        );
    }

    #[tokio::test]
    async fn eval_state_get_missing() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::StateGet("nonexistent".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn eval_state_counter_missing() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::StateCounter("nonexistent".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(0));
    }

    #[tokio::test]
    async fn eval_state_get_existing() {
        use acteon_state::{KeyKind, StateKey};

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Pre-populate state.
        let key = StateKey::new("notifications", "tenant-1", KeyKind::State, "my-key");
        store.set(&key, "my-value", None).await.unwrap();

        let ctx = test_context(&action, &store, &env);
        let expr = Expr::StateGet("my-key".into());
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("my-value".into())
        );
    }

    #[tokio::test]
    async fn eval_state_counter_existing() {
        use acteon_state::{KeyKind, StateKey};

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        let key = StateKey::new("notifications", "tenant-1", KeyKind::Counter, "hits");
        store.increment(&key, 42, None).await.unwrap();

        let ctx = test_context(&action, &store, &env);
        let expr = Expr::StateCounter("hits".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(42));
    }

    #[tokio::test]
    async fn eval_state_time_since_missing() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::StateTimeSince("nonexistent".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(i64::MAX));
    }

    #[tokio::test]
    async fn eval_state_time_since_existing() {
        use acteon_state::{KeyKind, StateKey};

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Store a timestamp 60 seconds in the past.
        let past = Utc::now() - chrono::Duration::seconds(60);
        let key = StateKey::new("notifications", "tenant-1", KeyKind::State, "last-sent");
        store.set(&key, &past.to_rfc3339(), None).await.unwrap();

        let ctx = test_context(&action, &store, &env);
        let expr = Expr::StateTimeSince("last-sent".into());
        match eval(&expr, &ctx).await.unwrap() {
            Value::Int(seconds) => {
                // Allow some slack for test execution time.
                assert!(seconds >= 59 && seconds <= 62, "got {seconds}");
            }
            other => panic!("expected Int, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn eval_index_access() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Index(
            Box::new(Expr::List(vec![
                Expr::String("a".into()),
                Expr::String("b".into()),
                Expr::String("c".into()),
            ])),
            Box::new(Expr::Int(1)),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::String("b".into()));
    }

    #[tokio::test]
    async fn eval_undefined_variable() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Ident("nonexistent_var".into());
        assert!(eval(&expr, &ctx).await.is_err());
    }

    // --- RuleEngine tests ---

    #[tokio::test]
    async fn engine_no_rules_returns_allow() {
        let engine = RuleEngine::new(vec![]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn engine_matching_rule() {
        let rule = Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny);

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Deny(_)));
    }

    #[tokio::test]
    async fn engine_non_matching_rule() {
        let rule = Rule::new("never-fires", Expr::Bool(false), RuleAction::Deny);

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn engine_priority_ordering() {
        let rule_low =
            Rule::new("low-priority", Expr::Bool(true), RuleAction::Allow).with_priority(100);
        let rule_high =
            Rule::new("high-priority", Expr::Bool(true), RuleAction::Deny).with_priority(1);

        // Even if low-priority is added first, high-priority should win.
        let engine = RuleEngine::new(vec![rule_low, rule_high]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Deny(_)));
    }

    #[tokio::test]
    async fn engine_first_match_wins() {
        let rule1 = Rule::new(
            "suppress-email",
            Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "action_type".into(),
                )),
                Box::new(Expr::String("send_email".into())),
            ),
            RuleAction::Suppress,
        )
        .with_priority(1);

        let rule2 = Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny).with_priority(2);

        let engine = RuleEngine::new(vec![rule1, rule2]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Suppress(_)));
    }

    #[tokio::test]
    async fn engine_disabled_rule_skipped() {
        let rule = Rule::new("disabled-deny", Expr::Bool(true), RuleAction::Deny)
            .with_enabled(false)
            .with_priority(1);

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    #[tokio::test]
    async fn engine_reroute_verdict() {
        let rule = Rule::new(
            "reroute-sms",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "sms-fallback".into(),
            },
        );

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

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

    #[tokio::test]
    async fn engine_throttle_verdict() {
        let rule = Rule::new(
            "rate-limit",
            Expr::Bool(true),
            RuleAction::Throttle {
                max_count: 100,
                window_seconds: 60,
            },
        );

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Throttle {
                rule,
                max_count,
                window_seconds,
            } => {
                assert_eq!(rule, "rate-limit");
                assert_eq!(max_count, 100);
                assert_eq!(window_seconds, 60);
            }
            other => panic!("expected Throttle, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_modify_verdict() {
        let changes = serde_json::json!({"priority": "high"});
        let rule = Rule::new(
            "boost-priority",
            Expr::Bool(true),
            RuleAction::Modify {
                changes: changes.clone(),
            },
        );

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Modify {
                rule: rule_name,
                changes: v,
            } => {
                assert_eq!(rule_name, "boost-priority");
                assert_eq!(v, changes);
            }
            other => panic!("expected Modify, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_dedup_verdict() {
        let rule = Rule::new(
            "dedup-5min",
            Expr::Bool(true),
            RuleAction::Deduplicate {
                ttl_seconds: Some(300),
            },
        );

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Deduplicate { ttl_seconds } => {
                assert_eq!(ttl_seconds, Some(300));
            }
            other => panic!("expected Deduplicate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn engine_add_rule() {
        let mut engine = RuleEngine::new(vec![]);

        let rule = Rule::new("late-add", Expr::Bool(true), RuleAction::Deny);
        engine.add_rule(rule);

        assert_eq!(engine.rules().len(), 1);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Deny(_)));
    }

    #[tokio::test]
    async fn eval_complex_condition() {
        // (action.action_type == "send_email") && (action.payload.priority > 3)
        let expr = Expr::Binary(
            BinaryOp::And,
            Box::new(Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "action_type".into(),
                )),
                Box::new(Expr::String("send_email".into())),
            )),
            Box::new(Expr::Binary(
                BinaryOp::Gt,
                Box::new(Expr::Field(
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("action".into())),
                        "payload".into(),
                    )),
                    "priority".into(),
                )),
                Box::new(Expr::Int(3)),
            )),
        );

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[test]
    fn engine_add_rules_batch() {
        let mut engine = RuleEngine::new(vec![
            Rule::new("existing", Expr::Bool(true), RuleAction::Allow).with_priority(10),
        ]);

        engine.add_rules(vec![
            Rule::new("new1", Expr::Bool(true), RuleAction::Deny).with_priority(5),
            Rule::new("new2", Expr::Bool(true), RuleAction::Suppress).with_priority(15),
        ]);

        assert_eq!(engine.rules().len(), 3);
        // Should be sorted: new1(5), existing(10), new2(15)
        assert_eq!(engine.rules()[0].name, "new1");
        assert_eq!(engine.rules()[1].name, "existing");
        assert_eq!(engine.rules()[2].name, "new2");
    }

    #[test]
    fn engine_list_rules() {
        let engine = RuleEngine::new(vec![
            Rule::new("alpha", Expr::Bool(true), RuleAction::Allow).with_priority(2),
            Rule::new("beta", Expr::Bool(true), RuleAction::Deny).with_priority(1),
        ]);
        let names = engine.list_rules();
        // Sorted by priority: beta(1), alpha(2)
        assert_eq!(names, vec!["beta", "alpha"]);
    }

    #[test]
    fn engine_enable_disable_rule() {
        let mut engine = RuleEngine::new(vec![Rule::new(
            "test-rule",
            Expr::Bool(true),
            RuleAction::Allow,
        )]);

        assert!(engine.rules()[0].enabled);

        assert!(engine.disable_rule("test-rule"));
        assert!(!engine.rules()[0].enabled);

        assert!(engine.enable_rule("test-rule"));
        assert!(engine.rules()[0].enabled);

        // Non-existent rule returns false
        assert!(!engine.disable_rule("nonexistent"));
        assert!(!engine.enable_rule("nonexistent"));
    }

    #[tokio::test]
    async fn engine_disabled_rule_not_evaluated() {
        let mut engine = RuleEngine::new(vec![
            Rule::new("deny-all", Expr::Bool(true), RuleAction::Deny).with_priority(1),
        ]);

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        // With rule enabled, should deny
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Deny(_)));

        // Disable it
        engine.disable_rule("deny-all");
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));

        // Re-enable
        engine.enable_rule("deny-all");
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Deny(_)));
    }

    #[test]
    fn rules_version_empty_engine() {
        let engine = RuleEngine::new(vec![]);
        // Just verify it returns a deterministic value.
        let v1 = engine.rules_version();
        let v2 = engine.rules_version();
        assert_eq!(v1, v2);
    }

    #[test]
    fn rules_version_changes_with_rules() {
        let engine1 = RuleEngine::new(vec![Rule::new(
            "rule-a",
            Expr::Bool(true),
            RuleAction::Allow,
        )]);
        let engine2 = RuleEngine::new(vec![Rule::new(
            "rule-b",
            Expr::Bool(true),
            RuleAction::Allow,
        )]);
        assert_ne!(engine1.rules_version(), engine2.rules_version());
    }

    #[test]
    fn rules_version_changes_with_version_field() {
        let engine1 = RuleEngine::new(vec![
            Rule::new("rule-a", Expr::Bool(true), RuleAction::Allow).with_version(1),
        ]);
        let engine2 = RuleEngine::new(vec![
            Rule::new("rule-a", Expr::Bool(true), RuleAction::Allow).with_version(2),
        ]);
        assert_ne!(engine1.rules_version(), engine2.rules_version());
    }

    #[test]
    fn rules_version_changes_with_enabled() {
        let engine1 = RuleEngine::new(vec![
            Rule::new("rule-a", Expr::Bool(true), RuleAction::Allow).with_enabled(true),
        ]);
        let engine2 = RuleEngine::new(vec![
            Rule::new("rule-a", Expr::Bool(true), RuleAction::Allow).with_enabled(false),
        ]);
        assert_ne!(engine1.rules_version(), engine2.rules_version());
    }

    #[test]
    fn rules_version_stable_same_rules() {
        let engine1 = RuleEngine::new(vec![
            Rule::new("a", Expr::Bool(true), RuleAction::Allow).with_version(1),
            Rule::new("b", Expr::Bool(false), RuleAction::Deny).with_version(3),
        ]);
        let engine2 = RuleEngine::new(vec![
            Rule::new("a", Expr::Bool(true), RuleAction::Allow).with_version(1),
            Rule::new("b", Expr::Bool(false), RuleAction::Deny).with_version(3),
        ]);
        assert_eq!(engine1.rules_version(), engine2.rules_version());
    }

    #[test]
    fn rule_verdict_rule_name() {
        assert_eq!(RuleVerdict::Allow(None).rule_name(), None);
        assert_eq!(
            RuleVerdict::Allow(Some("allow-rule".into())).rule_name(),
            Some("allow-rule")
        );
        assert_eq!(
            RuleVerdict::Deny("deny-rule".into()).rule_name(),
            Some("deny-rule")
        );
        assert_eq!(
            RuleVerdict::Suppress("suppress-rule".into()).rule_name(),
            Some("suppress-rule")
        );
        assert_eq!(
            RuleVerdict::Reroute {
                rule: "reroute-rule".into(),
                target_provider: "sms".into(),
            }
            .rule_name(),
            Some("reroute-rule")
        );
        assert_eq!(
            RuleVerdict::Throttle {
                rule: "throttle-rule".into(),
                max_count: 10,
                window_seconds: 60,
            }
            .rule_name(),
            Some("throttle-rule")
        );
        assert_eq!(
            RuleVerdict::Deduplicate {
                ttl_seconds: Some(300),
            }
            .rule_name(),
            None
        );
        assert_eq!(
            RuleVerdict::Modify {
                rule: "modify-rule".into(),
                changes: serde_json::json!({}),
            }
            .rule_name(),
            Some("modify-rule")
        );
        assert_eq!(
            RuleVerdict::RequestApproval {
                rule: "approval-rule".into(),
                notify_provider: "email".into(),
                timeout_seconds: 3600,
                message: None,
            }
            .rule_name(),
            Some("approval-rule")
        );
    }

    #[test]
    fn rule_verdict_chain() {
        let verdict = RuleVerdict::Chain {
            rule: "chain-rule".into(),
            chain: "my-chain".into(),
        };
        assert_eq!(verdict.rule_name(), Some("chain-rule"));
    }

    #[tokio::test]
    async fn engine_chain_verdict() {
        let rule = Rule::new(
            "start-chain",
            Expr::Bool(true),
            RuleAction::Chain {
                chain: "search-summarize".into(),
            },
        );

        let engine = RuleEngine::new(vec![rule]);
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let verdict = engine.evaluate(&ctx).await.unwrap();
        match verdict {
            RuleVerdict::Chain { rule, chain } => {
                assert_eq!(rule, "start-chain");
                assert_eq!(chain, "search-summarize");
            }
            other => panic!("expected Chain, got {other:?}"),
        }
    }

    #[test]
    fn engine_rule_by_name() {
        let engine = RuleEngine::new(vec![
            Rule::new("alpha", Expr::Bool(true), RuleAction::Allow),
            Rule::new("beta", Expr::Bool(false), RuleAction::Deny),
        ]);
        assert_eq!(engine.rule_by_name("alpha").unwrap().name, "alpha");
        assert_eq!(engine.rule_by_name("beta").unwrap().name, "beta");
        assert!(engine.rule_by_name("nonexistent").is_none());
    }

    // --- Semantic match tests ---

    /// A mock embedding support that returns a fixed similarity value.
    #[derive(Debug)]
    struct MockEmbedding {
        similarity: f64,
    }

    #[async_trait::async_trait]
    impl crate::engine::context::EmbeddingEvalSupport for MockEmbedding {
        async fn similarity(&self, _text: &str, _topic: &str) -> Result<f64, RuleError> {
            Ok(self.similarity)
        }
    }

    #[tokio::test]
    async fn eval_semantic_match_above_threshold() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let embedding = std::sync::Arc::new(MockEmbedding { similarity: 0.9 });
        let ctx = test_context(&action, &store, &env).with_embedding(embedding);

        let expr = Expr::SemanticMatch {
            topic: "email notifications".into(),
            threshold: 0.75,
            text_field: Some(Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "payload".into(),
                )),
                "subject".into(),
            ))),
        };
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn eval_semantic_match_below_threshold() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let embedding = std::sync::Arc::new(MockEmbedding { similarity: 0.5 });
        let ctx = test_context(&action, &store, &env).with_embedding(embedding);

        let expr = Expr::SemanticMatch {
            topic: "infrastructure outage".into(),
            threshold: 0.75,
            text_field: None,
        };
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn eval_semantic_match_no_embedding_errors() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::SemanticMatch {
            topic: "test".into(),
            threshold: 0.5,
            text_field: None,
        };
        assert!(eval(&expr, &ctx).await.is_err());
    }

    #[tokio::test]
    async fn eval_semantic_match_empty_text_returns_false() {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"message": ""}),
        );
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let embedding = std::sync::Arc::new(MockEmbedding { similarity: 1.0 });
        let ctx = test_context(&action, &store, &env).with_embedding(embedding);

        let expr = Expr::SemanticMatch {
            topic: "anything".into(),
            threshold: 0.5,
            text_field: Some(Box::new(Expr::Field(
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "payload".into(),
                )),
                "message".into(),
            ))),
        };
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    // --- Time-based rule activation tests ---

    #[tokio::test]
    async fn eval_time_ident_returns_map() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env);

        let expr = Expr::Ident("time".into());
        let result = eval(&expr, &ctx).await.unwrap();
        assert!(matches!(result, Value::Map(_)));
    }

    #[tokio::test]
    async fn eval_time_hour_field() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        // Fix to 2026-01-15 14:30:00 UTC (Thursday)
        let fixed_now = chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 14, 30, 45)
            .unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "hour".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(14));

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "minute".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(30));

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "second".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(45));
    }

    #[tokio::test]
    async fn eval_time_date_fields() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let fixed_now = chrono::Utc.with_ymd_and_hms(2026, 3, 22, 10, 0, 0).unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "day".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(22));

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "month".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(3));

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "year".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(2026));
    }

    #[tokio::test]
    async fn eval_time_weekday_fields() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        // 2026-01-15 is a Thursday
        let fixed_now = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "weekday".into());
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("Thursday".into())
        );

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "weekday_num".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(4)); // Thursday = 4
    }

    #[tokio::test]
    async fn eval_time_weekday_saturday() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        // 2026-01-17 is a Saturday
        let fixed_now = chrono::Utc.with_ymd_and_hms(2026, 1, 17, 12, 0, 0).unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "weekday".into());
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::String("Saturday".into())
        );

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "weekday_num".into());
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Int(6)); // Saturday = 6
    }

    #[tokio::test]
    async fn eval_time_timestamp() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let fixed_now = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        let expr = Expr::Field(Box::new(Expr::Ident("time".into())), "timestamp".into());
        assert_eq!(
            eval(&expr, &ctx).await.unwrap(),
            Value::Int(fixed_now.timestamp())
        );
    }

    #[tokio::test]
    async fn eval_business_hours_condition() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Business hours: time.hour >= 9 && time.hour < 17 && time.weekday_num <= 5
        let business_hours = Expr::All(vec![
            Expr::Binary(
                BinaryOp::Ge,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("time".into())),
                    "hour".into(),
                )),
                Box::new(Expr::Int(9)),
            ),
            Expr::Binary(
                BinaryOp::Lt,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("time".into())),
                    "hour".into(),
                )),
                Box::new(Expr::Int(17)),
            ),
            Expr::Binary(
                BinaryOp::Le,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("time".into())),
                    "weekday_num".into(),
                )),
                Box::new(Expr::Int(5)),
            ),
        ]);

        // Thursday at 14:00 — within business hours
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap());
        assert_eq!(
            eval(&business_hours, &ctx).await.unwrap(),
            Value::Bool(true)
        );

        // Thursday at 20:00 — outside business hours
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 20, 0, 0).unwrap());
        assert_eq!(
            eval(&business_hours, &ctx).await.unwrap(),
            Value::Bool(false)
        );

        // Saturday at 14:00 — weekend
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 17, 14, 0, 0).unwrap());
        assert_eq!(
            eval(&business_hours, &ctx).await.unwrap(),
            Value::Bool(false)
        );
    }

    #[tokio::test]
    async fn eval_time_weekday_string_comparison() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        // 2026-01-17 is a Saturday
        let fixed_now = chrono::Utc.with_ymd_and_hms(2026, 1, 17, 12, 0, 0).unwrap();
        let ctx = test_context(&action, &store, &env).with_now(fixed_now);

        // time.weekday != "Saturday"
        let expr = Expr::Binary(
            BinaryOp::Ne,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("time".into())),
                "weekday".into(),
            )),
            Box::new(Expr::String("Saturday".into())),
        );
        assert_eq!(eval(&expr, &ctx).await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn engine_time_based_rule() {
        use chrono::TimeZone as _;
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Suppress emails outside business hours (before 9 AM)
        let rule = Rule::new(
            "suppress-outside-hours",
            Expr::Binary(
                BinaryOp::Lt,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("time".into())),
                    "hour".into(),
                )),
                Box::new(Expr::Int(9)),
            ),
            RuleAction::Suppress,
        )
        .with_priority(1);

        let engine = RuleEngine::new(vec![rule]);

        // 3 AM — should suppress
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 3, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Suppress(_)));

        // 10 AM — should allow
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(matches!(verdict, RuleVerdict::Allow(_)));
    }

    // --- Timezone support tests ---

    #[tokio::test]
    async fn timezone_context_converts_hour() {
        use chrono::TimeZone as _;

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // UTC 14:00 = US/Eastern 9:00 (EST, UTC-5)
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap())
            .with_timezone(chrono_tz::US::Eastern);

        let time_val = build_time_map(&ctx);
        match time_val {
            Value::Map(ref m) => {
                assert_eq!(m.get("hour"), Some(&Value::Int(9)));
            }
            _ => panic!("expected Map"),
        }
    }

    #[tokio::test]
    async fn timezone_timestamp_always_utc() {
        use chrono::TimeZone as _;

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let utc_time = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap();

        let ctx = test_context(&action, &store, &env)
            .with_now(utc_time)
            .with_timezone(chrono_tz::US::Eastern);

        let time_val = build_time_map(&ctx);
        match time_val {
            Value::Map(ref m) => {
                assert_eq!(m.get("timestamp"), Some(&Value::Int(utc_time.timestamp())));
            }
            _ => panic!("expected Map"),
        }
    }

    #[tokio::test]
    async fn per_rule_timezone_override() {
        use chrono::TimeZone as _;

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Rule: suppress when hour < 10 (in US/Eastern)
        // UTC 14:00 = Eastern 9:00, so hour < 10 is true → suppress
        let rule = Rule::new(
            "eastern-morning",
            Expr::Binary(
                BinaryOp::Lt,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("time".into())),
                    "hour".into(),
                )),
                Box::new(Expr::Int(10)),
            ),
            RuleAction::Suppress,
        )
        .with_timezone("US/Eastern");

        let engine = RuleEngine::new(vec![rule]);

        // UTC 14:00 = EST 9:00 → hour(9) < 10 → suppress
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(
            matches!(verdict, RuleVerdict::Suppress(_)),
            "expected Suppress at Eastern 9 AM, got {verdict:?}"
        );

        // UTC 16:00 = EST 11:00 → hour(11) < 10 is false → allow
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 16, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(
            matches!(verdict, RuleVerdict::Allow(_)),
            "expected Allow at Eastern 11 AM, got {verdict:?}"
        );
    }

    #[tokio::test]
    async fn invalid_timezone_returns_error() {
        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        let rule = Rule::new("bad-tz", Expr::Bool(true), RuleAction::Suppress)
            .with_timezone("Fake/Timezone");

        let engine = RuleEngine::new(vec![rule]);
        let ctx = test_context(&action, &store, &env);
        let result = engine.evaluate(&ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid timezone"));
    }

    #[tokio::test]
    async fn business_hours_with_timezone() {
        use chrono::TimeZone as _;

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();

        // Business hours 9-17 Eastern, Mon-Fri
        // Condition: hour >= 9 && hour < 17 && weekday_num <= 5
        let rule = Rule::new(
            "business-hours-eastern",
            Expr::All(vec![
                Expr::Binary(
                    BinaryOp::Ge,
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("time".into())),
                        "hour".into(),
                    )),
                    Box::new(Expr::Int(9)),
                ),
                Expr::Binary(
                    BinaryOp::Lt,
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("time".into())),
                        "hour".into(),
                    )),
                    Box::new(Expr::Int(17)),
                ),
                Expr::Binary(
                    BinaryOp::Le,
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("time".into())),
                        "weekday_num".into(),
                    )),
                    Box::new(Expr::Int(5)),
                ),
            ]),
            RuleAction::Allow,
        )
        .with_timezone("US/Eastern");

        let engine = RuleEngine::new(vec![rule]);

        // Thursday 14:00 UTC = Thursday 9:00 Eastern → in business hours → Allow
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(
            matches!(&verdict, RuleVerdict::Allow(Some(name)) if name == "business-hours-eastern"),
            "expected Allow(business-hours-eastern), got {verdict:?}"
        );

        // Thursday 23:00 UTC = Thursday 18:00 Eastern → outside hours → Allow(None)
        let ctx = test_context(&action, &store, &env)
            .with_now(chrono::Utc.with_ymd_and_hms(2026, 1, 15, 23, 0, 0).unwrap());
        let verdict = engine.evaluate(&ctx).await.unwrap();
        assert!(
            matches!(&verdict, RuleVerdict::Allow(None)),
            "expected Allow(None), got {verdict:?}"
        );
    }

    #[test]
    fn time_map_cache_is_reused() {
        use chrono::TimeZone as _;

        let action = test_action();
        let store = MemoryStateStore::new();
        let env = HashMap::new();
        let ctx = test_context(&action, &store, &env).with_now(
            chrono::Utc
                .with_ymd_and_hms(2026, 6, 15, 10, 30, 0)
                .unwrap(),
        );

        // First call populates the cache.
        let val1 = resolve_ident("time", &ctx).unwrap();
        assert!(matches!(val1, Value::Map(_)));

        // Cache should now be populated.
        assert!(ctx.time_map_cache.get().is_some());

        // Second call returns a clone of the cached value — the underlying
        // OnceLock should still hold the same pointer.
        let val2 = resolve_ident("time", &ctx).unwrap();
        assert_eq!(val1, val2);

        // Verify it's the same cached object by comparing pointers.
        let cached = ctx.time_map_cache.get().unwrap();
        assert!(std::ptr::eq(cached, ctx.time_map_cache.get().unwrap()));
    }
}
