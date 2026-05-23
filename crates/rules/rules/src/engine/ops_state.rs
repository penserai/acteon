use acteon_state::{KeyKind, StateKey};

use crate::engine::context::EvalContext;
use crate::engine::value::Value;
use crate::error::RuleError;

/// Retrieve a state value by key pattern.
pub(crate) async fn eval_state_get(
    key_pattern: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    if let Some(ref tracker) = ctx.access_tracker {
        tracker.record_state_key(&format!("state:{key_pattern}"));
    }
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
pub(crate) async fn eval_state_counter(
    key_pattern: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    if let Some(ref tracker) = ctx.access_tracker {
        tracker.record_state_key(&format!("counter:{key_pattern}"));
    }
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
pub(crate) async fn eval_state_time_since(
    key_pattern: &str,
    ctx: &EvalContext<'_>,
) -> Result<Value, RuleError> {
    if let Some(ref tracker) = ctx.access_tracker {
        tracker.record_state_key(&format!("time_since:{key_pattern}"));
    }
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
