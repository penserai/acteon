//! Audit record construction helpers.
//!
//! Free functions extracted from the main gateway module to keep
//! `gateway.rs` focused on dispatch orchestration.

use std::time::Duration;

use chrono::Utc;

use acteon_audit::{A2A_AUDIT_PROVIDER, AuditEventKind, AuditRecord};
use acteon_core::{Action, ActionOutcome, Caller, Task, TaskState};
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

/// Like [`matched_rule_name`] but also surfaces the rule name attached to
/// `RuleVerdict::Allow(Some(name))`. The audit helper hides the name on
/// allow verdicts to indicate "no rule changed the outcome", but the
/// time-interval gate still needs the actual matched rule so it can read
/// its `mute_time_intervals` / `active_time_intervals` references.
pub(crate) fn rule_name_for_lookup(verdict: &RuleVerdict) -> Option<String> {
    match verdict {
        RuleVerdict::Allow(name) => name.clone(),
        RuleVerdict::Deduplicate { .. } => None,
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
        ActionOutcome::Silenced { .. } => "silenced",
        ActionOutcome::Muted { .. } => "muted",
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

/// Build an [`AuditRecord`] for an A2A Task lifecycle event.
///
/// A2A Task transitions are not rule-evaluated provider dispatches, so
/// the action-centric fields are mapped onto the Task domain: `action_id`
/// is the task id, `provider` is the synthetic [`A2A_AUDIT_PROVIDER`]
/// marker, `action_type` is the [`AuditEventKind::A2aTaskTransition`]
/// discriminator, and the lifecycle detail (operation, from/to state)
/// lands in `outcome_details`. Routing it through [`AuditRecord`] means
/// every Task event inherits the same hash-chain and compliance
/// machinery as action records.
///
/// `caller` is `None` for system-driven events (e.g. the stale-task
/// reaper); externally-driven transitions stamp the caller once the
/// protocol-codec layer threads identity into the engine.
pub(crate) fn build_task_audit_record(
    task: &Task,
    operation: &str,
    from_state: Option<TaskState>,
    occurred_at: chrono::DateTime<chrono::Utc>,
    caller: Option<&Caller>,
) -> AuditRecord {
    let to_state = task.status.state;
    let outcome_details = serde_json::json!({
        "operation": operation,
        "from_state": from_state.map(TaskState::as_str),
        "to_state": to_state.as_str(),
        "context_id": task.context_id,
        "pending_approval_id": task.pending_approval_id,
        "history_len": task.history.len(),
        "artifact_count": task.artifacts.len(),
    });

    AuditRecord {
        id: uuid::Uuid::now_v7().to_string(),
        action_id: task.id.clone(),
        chain_id: None,
        namespace: task.namespace.clone(),
        tenant: task.tenant.clone(),
        provider: A2A_AUDIT_PROVIDER.to_owned(),
        action_type: AuditEventKind::A2aTaskTransition
            .as_action_type()
            .to_owned(),
        // Task transitions are not gated by rule evaluation; record a
        // stable neutral verdict so analytics that group on it don't
        // see an empty bucket.
        verdict: "allow".to_owned(),
        matched_rule: None,
        outcome: to_state.as_str().to_owned(),
        action_payload: None,
        verdict_details: serde_json::json!({}),
        outcome_details,
        metadata: serde_json::to_value(&task.metadata).unwrap_or_default(),
        dispatched_at: occurred_at,
        completed_at: occurred_at,
        duration_ms: 0,
        expires_at: None,
        caller_id: caller.map_or_else(String::new, |c| c.id.clone()),
        auth_method: caller.map_or_else(String::new, |c| c.auth_method.clone()),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        kid: None,
        canonical_hash: None,
    }
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
        ActionOutcome::Silenced {
            silence_id,
            matched_rule,
        } => serde_json::json!({
            "silence_id": silence_id,
            "matched_rule": matched_rule,
        }),
        ActionOutcome::Muted {
            interval,
            reason,
            matched_rule,
        } => serde_json::json!({
            "interval": interval,
            "reason": reason,
            "matched_rule": matched_rule,
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
            let padding = a
                .data_base64
                .as_bytes()
                .iter()
                .rev()
                .take_while(|&&b| b == b'=')
                .count();
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
        signature: action.signature.clone(),
        signer_id: action.signer_id.clone(),
        kid: action.kid.clone(),
        canonical_hash: if action.signature.is_some() {
            // Compute SHA-256 of canonical bytes at audit time so the
            // verify endpoint can check without reconstructing the
            // full action (which audit records don't carry entirely).
            use sha2::Digest;
            let hash = sha2::Sha256::digest(action.canonical_bytes());
            Some(hex::encode(hash))
        } else {
            None
        },
    }
}

/// Build a **pre-execution intent** audit record: a durable "about to handle
/// this action with this verdict" marker written *before* the provider side
/// effect. In compliance mode the gateway writes this synchronously and fails
/// closed if it cannot be persisted — so an action that can't be recorded is
/// never executed. Mirrors [`build_audit_record`] but carries a synthetic
/// `pending` outcome (no result is known yet); the matching outcome record is
/// appended after execution.
#[allow(clippy::cast_possible_wrap)]
pub(crate) fn build_intent_audit_record(
    id: String,
    action: &Action,
    verdict: &RuleVerdict,
    dispatched_at: chrono::DateTime<chrono::Utc>,
    ttl_seconds: Option<u64>,
    store_payload: bool,
    caller: Option<&Caller>,
) -> AuditRecord {
    let expires_at = ttl_seconds.map(|secs| dispatched_at + chrono::Duration::seconds(secs as i64));
    let action_payload = if store_payload {
        Some(action.payload.clone())
    } else {
        None
    };
    AuditRecord {
        id,
        action_id: action.id.to_string(),
        chain_id: None,
        namespace: action.namespace.to_string(),
        tenant: action.tenant.to_string(),
        provider: action.provider.to_string(),
        action_type: action.action_type.clone(),
        verdict: verdict.as_tag().to_owned(),
        matched_rule: matched_rule_name(verdict),
        outcome: acteon_audit::INTENT_OUTCOME.to_owned(),
        action_payload,
        verdict_details: serde_json::json!({ "verdict": verdict.as_tag() }),
        outcome_details: serde_json::json!({ "phase": "intent" }),
        metadata: enrich_audit_metadata(action),
        dispatched_at,
        completed_at: dispatched_at,
        duration_ms: 0,
        expires_at,
        caller_id: caller.map_or_else(String::new, |c| c.id.clone()),
        auth_method: caller.map_or_else(String::new, |c| c.auth_method.clone()),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: action.signature.clone(),
        signer_id: action.signer_id.clone(),
        kid: action.kid.clone(),
        canonical_hash: if action.signature.is_some() {
            use sha2::Digest;
            let hash = sha2::Sha256::digest(action.canonical_bytes());
            Some(hex::encode(hash))
        } else {
            None
        },
    }
}
