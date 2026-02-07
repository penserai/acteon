use acteon_state::{KeyKind, StateKey};

use crate::engine::context::EvalContext;
use crate::engine::eval::eval;
use crate::engine::value::Value;
use crate::error::RuleError;
use crate::ir::expr::Expr;

/// Check if an active event exists with the given type and optional label value.
///
/// This is used for inhibition: suppressing alerts when a parent alert is active.
/// For example, suppress pod alerts when a `cluster_down` event is active.
pub(crate) async fn eval_has_active_event(
    event_type: &str,
    label_value_expr: Option<&Expr>,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    // Build the key for looking up active events
    // Format: {ns}:{tenant}:active_events:{type}:{hash}
    // where hash is derived from the label value if provided

    let label_suffix = if let Some(expr) = label_value_expr {
        let val = Box::pin(eval(expr, ctx)).await?;
        match val {
            Value::String(s) => format!(":{s}"),
            Value::Null => String::new(),
            other => format!(":{}", other.display_string()),
        }
    } else {
        String::new()
    };

    let key_id = format!("{event_type}{label_suffix}");
    let state_key = StateKey::new(
        ctx.action.namespace.as_str(),
        ctx.action.tenant.as_str(),
        KeyKind::ActiveEvents,
        &key_id,
    );

    match ctx.state.get(&state_key).await {
        Ok(Some(val)) => {
            // Check if the stored state indicates an active (non-resolved) event
            // We store JSON like: {"state": "firing", "fingerprint": "..."}
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&val)
                && let Some(state) = parsed.get("state").and_then(|s| s.as_str())
            {
                // Consider "resolved" and "closed" as inactive
                let is_active = !matches!(state, "resolved" | "closed");
                return Ok(Value::Bool(is_active));
            }
            // If we can't parse, assume it's active
            Ok(Value::Bool(true))
        }
        Ok(None) => Ok(Value::Bool(false)),
        Err(e) => Err(RuleError::StateAccess(e.to_string())),
    }
}

/// Get the current state of an event by fingerprint.
pub(crate) async fn eval_get_event_state(
    fingerprint_expr: &Expr,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    let fingerprint = Box::pin(eval(fingerprint_expr, ctx)).await?;
    let fingerprint_str = match fingerprint {
        Value::String(s) => s,
        Value::Null => return Ok(Value::Null),
        other => other.display_string(),
    };

    let state_key = StateKey::new(
        ctx.action.namespace.as_str(),
        ctx.action.tenant.as_str(),
        KeyKind::EventState,
        &fingerprint_str,
    );

    match ctx.state.get(&state_key).await {
        Ok(Some(val)) => {
            // The stored value may be JSON with state info or just the state name
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&val)
                && let Some(state) = parsed.get("state").and_then(|s| s.as_str())
            {
                return Ok(Value::String(state.to_string()));
            }
            // If not JSON, return the raw value as the state
            Ok(Value::String(val))
        }
        Ok(None) => Ok(Value::Null),
        Err(e) => Err(RuleError::StateAccess(e.to_string())),
    }
}

/// Check if an event is in a specific state.
pub(crate) async fn eval_event_in_state(
    fingerprint_expr: &Expr,
    expected_state: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    let current_state = eval_get_event_state(fingerprint_expr, ctx).await?;
    match current_state {
        Value::String(s) => Ok(Value::Bool(s == expected_state)),
        _ => Ok(Value::Bool(false)),
    }
}
