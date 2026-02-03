use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use acteon_state::{KeyKind, StateKey};

use crate::engine::builtins::call_builtin;
use crate::engine::context::EvalContext;
use crate::error::RuleError;
use crate::ir::expr::{BinaryOp, Expr, UnaryOp};
use crate::ir::rule::{Rule, RuleAction};

/// Runtime value produced by expression evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// The null value.
    Null,
    /// A boolean value.
    Bool(bool),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit floating-point number.
    Float(f64),
    /// A UTF-8 string.
    String(String),
    /// An ordered list of values.
    List(Vec<Value>),
    /// A string-keyed map of values.
    Map(HashMap<String, Value>),
}

impl Value {
    /// Convert a `serde_json::Value` into a runtime `Value`.
    pub fn from_json(json: serde_json::Value) -> Self {
        match json {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Self::Float(f)
                } else {
                    Self::Null
                }
            }
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Array(arr) => {
                Self::List(arr.into_iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(obj) => Self::Map(
                obj.into_iter()
                    .map(|(k, v)| (k, Self::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// Returns `true` if this value is considered truthy.
    ///
    /// - `Null` is falsy.
    /// - `Bool` is its own truthiness.
    /// - `Int(0)` and `Float(0.0)` are falsy.
    /// - Empty strings, lists, and maps are falsy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(n) => *n != 0,
            Self::Float(f) => *f != 0.0,
            Self::String(s) => !s.is_empty(),
            Self::List(v) => !v.is_empty(),
            Self::Map(m) => !m.is_empty(),
        }
    }

    /// Returns a string representation of the value type.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Map(_) => "map",
        }
    }

    /// Returns a human-readable display string for the value.
    pub fn display_string(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Int(n) => n.to_string(),
            Self::Float(f) => f.to_string(),
            Self::String(s) => s.clone(),
            Self::List(v) => format!("{v:?}"),
            Self::Map(m) => format!("{m:?}"),
        }
    }

    /// Access a field by name on this value (for Map values).
    fn field(&self, name: &str) -> Result<Self, RuleError> {
        match self {
            Self::Map(m) => Ok(m.get(name).cloned().unwrap_or(Self::Null)),
            _ => Err(RuleError::TypeError(format!(
                "cannot access field '{name}' on {}",
                self.type_name()
            ))),
        }
    }

    /// Access an index on this value (for List and Map values).
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    fn index(&self, idx: &Self) -> Result<Self, RuleError> {
        match (self, idx) {
            (Self::List(v), Self::Int(i)) => {
                let index = if *i < 0 {
                    (v.len() as i64 + i) as usize
                } else {
                    *i as usize
                };
                Ok(v.get(index).cloned().unwrap_or(Self::Null))
            }
            (Self::Map(m), Self::String(key)) => Ok(m.get(key).cloned().unwrap_or(Self::Null)),
            _ => Err(RuleError::TypeError(format!(
                "cannot index {} with {}",
                self.type_name(),
                idx.type_name()
            ))),
        }
    }
}

/// Recursively evaluate an expression against the provided context.
pub async fn eval(expr: &Expr, ctx: &EvalContext<'_>) -> Result<Value, RuleError> {
    match expr {
        Expr::Null => Ok(Value::Null),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Float(f) => Ok(Value::Float(*f)),
        Expr::String(s) => Ok(Value::String(s.clone())),

        Expr::List(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(Box::pin(eval(item, ctx)).await?);
            }
            Ok(Value::List(result))
        }

        Expr::Map(entries) => {
            let mut result = HashMap::with_capacity(entries.len());
            for (key, value) in entries {
                result.insert(key.clone(), Box::pin(eval(value, ctx)).await?);
            }
            Ok(Value::Map(result))
        }

        Expr::Ident(name) => resolve_ident(name, ctx),

        Expr::Field(base, field) => {
            let base_val = Box::pin(eval(base, ctx)).await?;
            base_val.field(field)
        }

        Expr::Index(base, index) => {
            let base_val = Box::pin(eval(base, ctx)).await?;
            let index_val = Box::pin(eval(index, ctx)).await?;
            base_val.index(&index_val)
        }

        Expr::Unary(op, inner) => {
            let val = Box::pin(eval(inner, ctx)).await?;
            eval_unary(*op, &val)
        }

        Expr::Binary(op, lhs, rhs) => eval_binary(*op, lhs, rhs, ctx).await,

        Expr::Ternary(cond, then_branch, else_branch) => {
            let cond_val = Box::pin(eval(cond, ctx)).await?;
            if cond_val.is_truthy() {
                Box::pin(eval(then_branch, ctx)).await
            } else {
                Box::pin(eval(else_branch, ctx)).await
            }
        }

        Expr::Call(name, args) => {
            let mut evaluated_args = Vec::with_capacity(args.len());
            for arg in args {
                evaluated_args.push(Box::pin(eval(arg, ctx)).await?);
            }
            call_builtin(name, &evaluated_args)
        }

        Expr::All(exprs) => {
            for e in exprs {
                let val = Box::pin(eval(e, ctx)).await?;
                if !val.is_truthy() {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }

        Expr::Any(exprs) => {
            for e in exprs {
                let val = Box::pin(eval(e, ctx)).await?;
                if val.is_truthy() {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }

        Expr::StateGet(key_pattern) => eval_state_get(key_pattern, ctx).await,
        Expr::StateCounter(key_pattern) => eval_state_counter(key_pattern, ctx).await,
        Expr::StateTimeSince(key_pattern) => eval_state_time_since(key_pattern, ctx).await,
    }
}

/// Resolve a top-level identifier to a value from the evaluation context.
fn resolve_ident(name: &str, ctx: &EvalContext<'_>) -> Result<Value, RuleError> {
    match name {
        "action" => {
            let json = serde_json::to_value(ctx.action)
                .map_err(|e| RuleError::Evaluation(format!("failed to serialize action: {e}")))?;
            Ok(Value::from_json(json))
        }
        "env" | "environment" => {
            let map: HashMap<String, Value> = ctx
                .environment
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            Ok(Value::Map(map))
        }
        "now" => Ok(Value::Int(ctx.now.timestamp())),
        _ => {
            // Try environment lookup as a shortcut.
            if let Some(val) = ctx.environment.get(name) {
                return Ok(Value::String(val.clone()));
            }
            Err(RuleError::UndefinedVariable(name.to_owned()))
        }
    }
}

/// Evaluate a unary operation on a value.
fn eval_unary(op: UnaryOp, val: &Value) -> Result<Value, RuleError> {
    match op {
        UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
        UnaryOp::Neg => match val {
            Value::Int(n) => Ok(Value::Int(-n)),
            Value::Float(f) => Ok(Value::Float(-f)),
            _ => Err(RuleError::TypeError(format!(
                "cannot negate {}",
                val.type_name()
            ))),
        },
    }
}

/// Evaluate a binary operation with short-circuit semantics for And/Or.
async fn eval_binary(
    op: BinaryOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    // Short-circuit for logical operators.
    match op {
        BinaryOp::And => {
            let left = Box::pin(eval(lhs, ctx)).await?;
            if !left.is_truthy() {
                return Ok(Value::Bool(false));
            }
            let right = Box::pin(eval(rhs, ctx)).await?;
            return Ok(Value::Bool(right.is_truthy()));
        }
        BinaryOp::Or => {
            let left = Box::pin(eval(lhs, ctx)).await?;
            if left.is_truthy() {
                return Ok(Value::Bool(true));
            }
            let right = Box::pin(eval(rhs, ctx)).await?;
            return Ok(Value::Bool(right.is_truthy()));
        }
        _ => {}
    }

    let left = Box::pin(eval(lhs, ctx)).await?;
    let right = Box::pin(eval(rhs, ctx)).await?;

    match op {
        // Arithmetic
        BinaryOp::Add => eval_add(&left, &right),
        BinaryOp::Sub => eval_arithmetic(&left, &right, |a, b| a - b, |a, b| a - b, "subtract"),
        BinaryOp::Mul => eval_arithmetic(&left, &right, |a, b| a * b, |a, b| a * b, "multiply"),
        BinaryOp::Div => eval_div(&left, &right),
        BinaryOp::Mod => eval_mod(&left, &right),

        // Comparison
        BinaryOp::Eq => Ok(Value::Bool(values_equal(&left, &right))),
        BinaryOp::Ne => Ok(Value::Bool(!values_equal(&left, &right))),
        BinaryOp::Lt => eval_compare(&left, &right, std::cmp::Ordering::is_lt),
        BinaryOp::Le => eval_compare(&left, &right, std::cmp::Ordering::is_le),
        BinaryOp::Gt => eval_compare(&left, &right, std::cmp::Ordering::is_gt),
        BinaryOp::Ge => eval_compare(&left, &right, std::cmp::Ordering::is_ge),

        // String operations
        BinaryOp::Contains => eval_contains(&left, &right),
        BinaryOp::StartsWith => eval_starts_with(&left, &right),
        BinaryOp::EndsWith => eval_ends_with(&left, &right),
        BinaryOp::Matches => eval_matches(&left, &right),
        BinaryOp::In => eval_in(&left, &right),

        // Already handled above, but needed for exhaustiveness.
        BinaryOp::And | BinaryOp::Or => unreachable!(),
    }
}

/// Add two values (supports int, float, and string concatenation).
#[allow(clippy::cast_precision_loss)]
fn eval_add(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_add(*b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
        _ => Err(RuleError::TypeError(format!(
            "cannot add {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Generic arithmetic on two numeric values.
#[allow(clippy::cast_precision_loss)]
fn eval_arithmetic(
    left: &Value,
    right: &Value,
    int_op: fn(i64, i64) -> i64,
    float_op: fn(f64, f64) -> f64,
    op_name: &str,
) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(*a, *b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(*a, *b))),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(*a as f64, *b))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(*a, *b as f64))),
        _ => Err(RuleError::TypeError(format!(
            "cannot {op_name} {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Division with zero-check.
#[allow(clippy::cast_precision_loss)]
fn eval_div(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::Int(_) | Value::Float(_), Value::Int(0)) => {
            Err(RuleError::Evaluation("division by zero".into()))
        }
        (Value::Int(_) | Value::Float(_), Value::Float(f)) if *f == 0.0 => {
            Err(RuleError::Evaluation("division by zero".into()))
        }
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a / b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 / b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / *b as f64)),
        _ => Err(RuleError::TypeError(format!(
            "cannot divide {} by {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Modulo with zero-check.
#[allow(clippy::cast_precision_loss)]
fn eval_mod(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::Int(_), Value::Int(0)) => Err(RuleError::Evaluation("modulo by zero".into())),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a % b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 % b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a % *b as f64)),
        _ => Err(RuleError::TypeError(format!(
            "cannot modulo {} by {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Check equality of two values, with type coercion for int/float.
#[allow(clippy::cast_precision_loss)]
fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => (a - b).abs() < f64::EPSILON,
        (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
            (*a as f64 - b).abs() < f64::EPSILON
        }
        (Value::String(a), Value::String(b)) => a == b,
        (Value::List(a), Value::List(b)) => a == b,
        _ => false,
    }
}

/// Ordered comparison returning the `Ordering`.
#[allow(clippy::cast_precision_loss)]
fn eval_compare(
    left: &Value,
    right: &Value,
    predicate: fn(std::cmp::Ordering) -> bool,
) -> Result<Value, RuleError> {
    let ordering = match (left, right) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Int(a), Value::Float(b)) => (*a as f64)
            .partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::Float(a), Value::Int(b)) => a
            .partial_cmp(&(*b as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        _ => {
            return Err(RuleError::TypeError(format!(
                "cannot compare {} and {}",
                left.type_name(),
                right.type_name()
            )));
        }
    };
    Ok(Value::Bool(predicate(ordering)))
}

/// String contains check.
fn eval_contains(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::String(haystack), Value::String(needle)) => {
            Ok(Value::Bool(haystack.contains(needle.as_str())))
        }
        (Value::List(list), needle) => Ok(Value::Bool(list.contains(needle))),
        _ => Err(RuleError::TypeError(format!(
            "contains: unsupported types {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// String `starts_with` check.
fn eval_starts_with(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::String(s), Value::String(prefix)) => {
            Ok(Value::Bool(s.starts_with(prefix.as_str())))
        }
        _ => Err(RuleError::TypeError(format!(
            "starts_with: unsupported types {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// String `ends_with` check.
fn eval_ends_with(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::String(s), Value::String(suffix)) => Ok(Value::Bool(s.ends_with(suffix.as_str()))),
        _ => Err(RuleError::TypeError(format!(
            "ends_with: unsupported types {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Regex matches check.
fn eval_matches(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match (left, right) {
        (Value::String(s), Value::String(pattern)) => {
            let re = Regex::new(pattern).map_err(|e| RuleError::InvalidRegex(e.to_string()))?;
            Ok(Value::Bool(re.is_match(s)))
        }
        _ => Err(RuleError::TypeError(format!(
            "matches: unsupported types {} and {}",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Membership test: `value in collection`.
fn eval_in(left: &Value, right: &Value) -> Result<Value, RuleError> {
    match right {
        Value::List(list) => Ok(Value::Bool(list.contains(left))),
        Value::Map(map) => match left {
            Value::String(key) => Ok(Value::Bool(map.contains_key(key))),
            _ => Err(RuleError::TypeError(format!(
                "in: map key must be string, got {}",
                left.type_name()
            ))),
        },
        Value::String(s) => match left {
            Value::String(sub) => Ok(Value::Bool(s.contains(sub.as_str()))),
            _ => Err(RuleError::TypeError(format!(
                "in: cannot check {} membership in string",
                left.type_name()
            ))),
        },
        _ => Err(RuleError::TypeError(format!(
            "in: right-hand side must be list, map, or string, got {}",
            right.type_name()
        ))),
    }
}

/// Retrieve a state value by key pattern.
async fn eval_state_get(key_pattern: &str, ctx: &EvalContext<'_>) -> Result<Value, RuleError> {
    let state_key = StateKey::new(
        ctx.action.namespace.as_str(),
        ctx.action.tenant.as_str(),
        KeyKind::State,
        key_pattern,
    );
    match ctx.state.get(&state_key).await {
        Ok(Some(val)) => Ok(Value::String(val)),
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RuleError::StateAccess(e.to_string())),
    }
}

/// Retrieve a counter value from the state store.
async fn eval_state_counter(key_pattern: &str, ctx: &EvalContext<'_>) -> Result<Value, RuleError> {
    let state_key = StateKey::new(
        ctx.action.namespace.as_str(),
        ctx.action.tenant.as_str(),
        KeyKind::Counter,
        key_pattern,
    );
    match ctx.state.get(&state_key).await {
        Ok(Some(val)) => {
            let n: i64 = val
                .parse()
                .map_err(|e| RuleError::StateAccess(format!("counter is not an integer: {e}")))?;
            Ok(Value::Int(n))
        }
        Ok(None) => Ok(Value::Int(0)),
        Err(e) => Err(RuleError::StateAccess(e.to_string())),
    }
}

/// Compute the duration (in seconds) since the last update for a given state key.
async fn eval_state_time_since(
    key_pattern: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    let state_key = StateKey::new(
        ctx.action.namespace.as_str(),
        ctx.action.tenant.as_str(),
        KeyKind::State,
        key_pattern,
    );
    match ctx.state.get(&state_key).await {
        Ok(Some(val)) => {
            // Expect the stored value to be an ISO-8601 timestamp.
            let stored_time = chrono::DateTime::parse_from_rfc3339(&val)
                .map_err(|e| {
                    RuleError::StateAccess(format!("cannot parse stored timestamp '{val}': {e}"))
                })?
                .with_timezone(&chrono::Utc);
            let elapsed = ctx.now.signed_duration_since(stored_time);
            Ok(Value::Int(elapsed.num_seconds()))
        }
        Ok(None) => {
            // No prior state: return a very large number to indicate "never".
            Ok(Value::Int(i64::MAX))
        }
        Err(e) => Err(RuleError::StateAccess(e.to_string())),
    }
}

/// The verdict produced by the rule engine after evaluating all rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleVerdict {
    /// Allow the action to proceed.
    Allow,
    /// Deny the action with a reason.
    Deny(String),
    /// Deduplicate with an optional TTL.
    Deduplicate {
        /// Time-to-live in seconds.
        ttl_seconds: Option<u64>,
    },
    /// Suppress the action with a reason.
    Suppress(String),
    /// Reroute to a different provider.
    Reroute {
        /// Name of the rule that triggered the reroute.
        rule: String,
        /// The target provider.
        target_provider: String,
    },
    /// Throttle the action.
    Throttle {
        /// Name of the rule that triggered throttling.
        rule: String,
        /// Maximum count in the window.
        max_count: u64,
        /// Window size in seconds.
        window_seconds: u64,
    },
    /// Modify the action.
    Modify {
        /// Name of the rule that triggered the modification.
        rule: String,
        /// The JSON changes to apply.
        changes: serde_json::Value,
    },
}

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
    /// rule matches.
    #[instrument(skip_all, fields(rules_count = self.rules.len()))]
    pub async fn evaluate(&self, ctx: &EvalContext<'_>) -> Result<RuleVerdict, RuleError> {
        for rule in &self.rules {
            if !rule.enabled {
                debug!(rule = %rule.name, "skipping disabled rule");
                continue;
            }

            let result = eval(&rule.condition, ctx).await?;

            if result.is_truthy() {
                debug!(rule = %rule.name, "rule matched");
                return Ok(action_to_verdict(&rule.name, &rule.action));
            }
        }

        debug!("no rules matched, returning Allow");
        Ok(RuleVerdict::Allow)
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

/// Convert a `RuleAction` into a `RuleVerdict` with the rule name attached.
fn action_to_verdict(rule_name: &str, action: &RuleAction) -> RuleVerdict {
    match action {
        RuleAction::Allow => RuleVerdict::Allow,
        RuleAction::Deny => RuleVerdict::Deny(rule_name.to_owned()),
        RuleAction::Deduplicate { ttl_seconds } => RuleVerdict::Deduplicate {
            ttl_seconds: *ttl_seconds,
        },
        RuleAction::Suppress => RuleVerdict::Suppress(rule_name.to_owned()),
        RuleAction::Reroute { target_provider } => RuleVerdict::Reroute {
            rule: rule_name.to_owned(),
            target_provider: target_provider.clone(),
        },
        RuleAction::Throttle {
            max_count,
            window_seconds,
        } => RuleVerdict::Throttle {
            rule: rule_name.to_owned(),
            max_count: *max_count,
            window_seconds: *window_seconds,
        },
        RuleAction::Modify { changes } => RuleVerdict::Modify {
            rule: rule_name.to_owned(),
            changes: changes.clone(),
        },
        RuleAction::Custom { name, params: _ } => {
            // Custom actions fall through as Allow for now, with a debug log.
            debug!(custom_action = %name, "custom action not handled, allowing");
            RuleVerdict::Allow
        }
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
        assert!(matches!(verdict, RuleVerdict::Allow));
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
        assert!(matches!(verdict, RuleVerdict::Allow));
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
        assert!(matches!(verdict, RuleVerdict::Allow));
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
        assert!(matches!(verdict, RuleVerdict::Allow));

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
}
