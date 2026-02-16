use std::collections::HashMap;

use regex::Regex;

use crate::engine::builtins::call_builtin;
use crate::engine::context::EvalContext;
use crate::engine::ops_event::{eval_event_in_state, eval_get_event_state, eval_has_active_event};
use crate::engine::ops_semantic::eval_semantic_match;
use crate::engine::ops_state::{eval_state_counter, eval_state_get, eval_state_time_since};
use crate::engine::value::Value;
use crate::error::RuleError;
use crate::ir::expr::{BinaryOp, Expr, UnaryOp};

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
            // Track environment key access for the playground trace.
            if let Some(ref tracker) = ctx.access_tracker
                && matches!(base.as_ref(), Expr::Ident(name) if name == "env" || name == "environment")
            {
                tracker.record_env_key(field);
            }
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

        Expr::HasActiveEvent {
            event_type,
            label_value,
        } => eval_has_active_event(event_type, label_value.as_deref(), ctx).await,
        Expr::GetEventState(fingerprint_expr) => eval_get_event_state(fingerprint_expr, ctx).await,
        Expr::EventInState { fingerprint, state } => {
            eval_event_in_state(fingerprint, state, ctx).await
        }

        Expr::WasmCall { plugin, function } => eval_wasm_call(plugin, function, ctx).await,

        Expr::SemanticMatch {
            topic,
            threshold,
            text_field,
        } => eval_semantic_match(topic, *threshold, text_field.as_deref(), ctx).await,
    }
}

/// Evaluate a WASM plugin call as a boolean condition.
async fn eval_wasm_call(
    plugin: &str,
    function: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    let runtime = ctx.wasm_runtime.as_ref().ok_or_else(|| {
        RuleError::Evaluation(format!(
            "WASM plugin '{plugin}' called but no WASM runtime configured"
        ))
    })?;

    // Serialize the action as the input payload for the plugin.
    let input = serde_json::to_value(ctx.action)
        .map_err(|e| RuleError::Evaluation(format!("failed to serialize action for WASM: {e}")))?;

    // Record the invocation attempt.
    if let Some(ref counters) = ctx.wasm_counters {
        counters.record_invocation();
    }

    let result = runtime
        .invoke(plugin, function, &input)
        .await
        .map_err(|e| {
            if let Some(ref counters) = ctx.wasm_counters {
                counters.record_error();
            }
            RuleError::Evaluation(format!("WASM plugin '{plugin}' error: {e}"))
        })?;

    Ok(Value::Bool(result.verdict))
}

/// Resolve a top-level identifier to a value from the evaluation context.
pub(crate) fn resolve_ident(name: &str, ctx: &EvalContext<'_>) -> Result<Value, RuleError> {
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
        "time" => Ok(ctx
            .time_map_cache
            .get_or_init(|| build_time_map(ctx))
            .clone()),
        _ => {
            // Try environment lookup as a shortcut.
            if let Some(val) = ctx.environment.get(name) {
                if let Some(ref tracker) = ctx.access_tracker {
                    tracker.record_env_key(name);
                }
                return Ok(Value::String(val.clone()));
            }
            Err(RuleError::UndefinedVariable(name.to_owned()))
        }
    }
}

/// Build a `Value::Map` containing temporal components derived from `ctx.now`.
///
/// When `ctx.timezone` is `Some(tz)`, date/time components (`hour`, `weekday`,
/// etc.) are computed in the given timezone. Otherwise they use UTC.
/// The `timestamp` field always returns the UTC unix timestamp regardless.
///
/// Provides the following fields for use in rule conditions:
/// - `hour` (0–23), `minute` (0–59), `second` (0–59)
/// - `day` (1–31), `month` (1–12), `year`
/// - `weekday` — English name (e.g. `"Monday"`)
/// - `weekday_num` — ISO weekday number (1=Monday … 7=Sunday)
/// - `timestamp` — Unix timestamp in seconds (always UTC)
pub(crate) fn build_time_map(ctx: &EvalContext<'_>) -> Value {
    use chrono::Datelike as _;
    use chrono::Timelike as _;

    // Extract date/time components in the configured timezone (or UTC).
    let (hour, minute, second, day, month, year, weekday) = if let Some(tz) = ctx.timezone {
        let local = ctx.now.with_timezone(&tz);
        (
            local.hour(),
            local.minute(),
            local.second(),
            local.day(),
            local.month(),
            local.year(),
            local.weekday(),
        )
    } else {
        let dt = ctx.now;
        (
            dt.hour(),
            dt.minute(),
            dt.second(),
            dt.day(),
            dt.month(),
            dt.year(),
            dt.weekday(),
        )
    };

    let weekday_name = match weekday {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    };

    let mut map = HashMap::with_capacity(9);
    map.insert("hour".to_owned(), Value::Int(i64::from(hour)));
    map.insert("minute".to_owned(), Value::Int(i64::from(minute)));
    map.insert("second".to_owned(), Value::Int(i64::from(second)));
    map.insert("day".to_owned(), Value::Int(i64::from(day)));
    map.insert("month".to_owned(), Value::Int(i64::from(month)));
    map.insert("year".to_owned(), Value::Int(i64::from(year)));
    map.insert("weekday".to_owned(), Value::String(weekday_name.to_owned()));
    map.insert(
        "weekday_num".to_owned(),
        Value::Int(i64::from(weekday.number_from_monday())),
    );
    // Timestamp is always UTC unix time.
    map.insert("timestamp".to_owned(), Value::Int(ctx.now.timestamp()));
    Value::Map(map)
}

/// Evaluate a unary operation on a value.
pub(crate) fn eval_unary(op: UnaryOp, val: &Value) -> Result<Value, RuleError> {
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
pub(crate) async fn eval_binary(
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
