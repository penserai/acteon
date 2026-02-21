//! Audit record construction helpers.
//!
//! Free functions extracted from the main gateway module to keep
//! `gateway.rs` focused on dispatch orchestration.

use std::time::Duration;

use chrono::Utc;

use acteon_audit::AuditRecord;
use acteon_core::{Action, ActionOutcome, Caller};
use acteon_rules::RuleVerdict;

/// Extract the matched rule name from a `RuleVerdict`, if any.
pub(crate) fn matched_rule_name(verdict: &RuleVerdict) -> Option<String> {
    match verdict {
        RuleVerdict::Allow(_) | RuleVerdict::Deduplicate { .. } => None,
        RuleVerdict::Deny(rule)
        | RuleVerdict::Suppress(rule)
        | RuleVerdict::Reroute { rule, .. }
        | RuleVerdict::Throttle { rule, .. }
        | RuleVerdict::Modify { rule, .. }
        | RuleVerdict::StateMachine { rule, .. }
        | RuleVerdict::Group { rule, .. }
        | RuleVerdict::RequestApproval { rule, .. }
        | RuleVerdict::Chain { rule, .. }
        | RuleVerdict::Schedule { rule, .. } => Some(rule.clone()),
    }
}

/// Extract a string tag from an `ActionOutcome`.
pub(crate) fn outcome_tag(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "executed",
        ActionOutcome::Deduplicated => "deduplicated",
        ActionOutcome::Suppressed { .. } => "suppressed",
        ActionOutcome::Rerouted { .. } => "rerouted",
        ActionOutcome::Throttled { .. } => "throttled",
        ActionOutcome::Failed(_) => "failed",
        ActionOutcome::Grouped { .. } => "grouped",
        ActionOutcome::StateChanged { .. } => "state_changed",
        ActionOutcome::PendingApproval { .. } => "pending_approval",
        ActionOutcome::ChainStarted { .. } => "chain_started",
        ActionOutcome::DryRun { .. } => "dry_run",
        ActionOutcome::CircuitOpen { .. } => "circuit_open",
        ActionOutcome::Scheduled { .. } => "scheduled",
        ActionOutcome::RecurringCreated { .. } => "recurring_created",
        ActionOutcome::QuotaExceeded { .. } => "quota_exceeded",
    }
}

/// Check if `source` and `target` are adjacent in the execution path
/// (i.e., `target` immediately follows `source`).
pub(crate) fn is_adjacent_in_path(path: &[String], source: &str, target: &str) -> bool {
    path.windows(2).any(|w| w[0] == source && w[1] == target)
}

/// Enrich serialized action metadata with extra `Action` fields so that
/// replays can reconstruct the full action. System fields use a `__` prefix
/// to distinguish them from user-supplied labels.
pub(crate) fn enrich_audit_metadata(action: &Action) -> serde_json::Value {
    let mut meta = serde_json::to_value(&action.metadata).unwrap_or_default();
    if let Some(obj) = meta.as_object_mut() {
        if let Some(k) = &action.dedup_key {
            obj.insert("__dedup_key".into(), serde_json::json!(k));
        }
        if let Some(f) = &action.fingerprint {
            obj.insert("__fingerprint".into(), serde_json::json!(f));
        }
        if let Some(s) = &action.status {
            obj.insert("__status".into(), serde_json::json!(s));
        }
        if let Some(t) = action.starts_at {
            obj.insert("__starts_at".into(), serde_json::json!(t));
        }
        if let Some(t) = action.ends_at {
            obj.insert("__ends_at".into(), serde_json::json!(t));
        }
    }
    meta
}

/// Build an `AuditRecord` from the dispatch context.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn build_audit_record(
    id: String,
    action: &Action,
    verdict: &RuleVerdict,
    outcome: &ActionOutcome,
    dispatched_at: chrono::DateTime<chrono::Utc>,
    elapsed: Duration,
    ttl_seconds: Option<u64>,
    store_payload: bool,
    caller: Option<&Caller>,
) -> AuditRecord {
    let completed_at = Utc::now();
    #[allow(clippy::cast_possible_wrap)]
    let expires_at = ttl_seconds.map(|secs| dispatched_at + chrono::Duration::seconds(secs as i64));

    let action_payload = if store_payload {
        Some(action.payload.clone())
    } else {
        None
    };

    let outcome_details = match outcome {
        ActionOutcome::Executed(resp) => serde_json::json!({
            "status": format!("{:?}", resp.status),
        }),
        ActionOutcome::Failed(err) => serde_json::json!({
            "code": err.code,
            "message": err.message,
            "retryable": err.retryable,
            "attempts": err.attempts,
        }),
        ActionOutcome::Suppressed { rule } => serde_json::json!({ "rule": rule }),
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            ..
        } => serde_json::json!({
            "original_provider": original_provider,
            "new_provider": new_provider,
        }),
        ActionOutcome::Throttled { retry_after } => {
            serde_json::json!({ "retry_after_secs": retry_after.as_secs() })
        }
        ActionOutcome::Deduplicated => serde_json::json!({}),
        ActionOutcome::Grouped {
            group_id,
            group_size,
            notify_at,
        } => serde_json::json!({
            "group_id": group_id,
            "group_size": group_size,
            "notify_at": notify_at.to_rfc3339(),
        }),
        ActionOutcome::StateChanged {
            fingerprint,
            previous_state,
            new_state,
            notify,
        } => serde_json::json!({
            "fingerprint": fingerprint,
            "previous_state": previous_state,
            "new_state": new_state,
            "notify": notify,
        }),
        ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            notification_sent,
            ..
        } => serde_json::json!({
            "approval_id": approval_id,
            "expires_at": expires_at.to_rfc3339(),
            "notification_sent": notification_sent,
        }),
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            first_step,
        } => serde_json::json!({
            "chain_id": chain_id,
            "chain_name": chain_name,
            "total_steps": total_steps,
            "first_step": first_step,
        }),
        ActionOutcome::DryRun {
            verdict,
            matched_rule,
            would_be_provider,
        } => serde_json::json!({
            "verdict": verdict,
            "matched_rule": matched_rule,
            "would_be_provider": would_be_provider,
        }),
        ActionOutcome::CircuitOpen {
            provider,
            fallback_chain,
        } => serde_json::json!({
            "provider": provider,
            "fallback_chain": fallback_chain,
        }),
        ActionOutcome::Scheduled {
            action_id,
            scheduled_for,
        } => serde_json::json!({
            "action_id": action_id,
            "scheduled_for": scheduled_for.to_rfc3339(),
        }),
        ActionOutcome::RecurringCreated {
            recurring_id,
            cron_expr,
            next_execution_at,
        } => serde_json::json!({
            "recurring_id": recurring_id,
            "cron_expr": cron_expr,
            "next_execution_at": next_execution_at.map(|t| t.to_rfc3339()),
        }),
        ActionOutcome::QuotaExceeded {
            tenant,
            limit,
            used,
            overage_behavior,
        } => serde_json::json!({
            "tenant": tenant,
            "limit": limit,
            "used": used,
            "overage_behavior": overage_behavior,
        }),
    };

    let chain_id = if let ActionOutcome::ChainStarted { chain_id, .. } = outcome {
        Some(chain_id.clone())
    } else {
        None
    };

    // Serialize attachment metadata (never binary data).
    // Compute the decoded binary size from the base64 length (each 4 base64
    // chars encode 3 bytes, minus padding).
    let attachment_metadata: Vec<serde_json::Value> = action
        .attachments
        .iter()
        .map(|a| {
            let b64_len = a.data_base64.len();
            let padding = a.data_base64.as_bytes().iter().rev().take_while(|&&b| b == b'=').count();
            let decoded_size = (b64_len / 4) * 3 - padding;
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "filename": a.filename,
                "content_type": a.content_type,
                "size_bytes": decoded_size,
            })
        })
        .collect();

    AuditRecord {
        id,
        action_id: action.id.to_string(),
        chain_id,
        namespace: action.namespace.to_string(),
        tenant: action.tenant.to_string(),
        provider: action.provider.to_string(),
        action_type: action.action_type.clone(),
        verdict: verdict.as_tag().to_owned(),
        matched_rule: matched_rule_name(verdict),
        outcome: outcome_tag(outcome).to_owned(),
        action_payload,
        verdict_details: serde_json::json!({ "verdict": verdict.as_tag() }),
        outcome_details,
        metadata: enrich_audit_metadata(action),
        dispatched_at,
        completed_at,
        duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        expires_at,
        caller_id: caller.map_or_else(String::new, |c| c.id.clone()),
        auth_method: caller.map_or_else(String::new, |c| c.auth_method.clone()),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata,
    }
}
