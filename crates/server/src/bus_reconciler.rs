//! Phase 10 Item 1 V2: background reconciler for stuck `Approving`
//! `BusApproval` rows.
//!
//! V1 (the state-machine refactor in `api/bus.rs`) closes the
//! visibility gap: a successful produce + failed CAS leaves the
//! row in `Approving`, not `Pending`. V2 (this module) automates
//! the retry — instead of asking an operator to call `approve`
//! again on every stuck row, a periodic sweep does it.
//!
//! The retry semantics are unchanged from the manual path:
//! re-produce with the same `acteon.tool.call_id` (so consumer-
//! side dedup catches duplicate Kafka records), CAS Approving →
//! Approved on success.
//!
//! Min age before retry: 30 s by default. That's well past
//! typical produce latency, so the reconciler doesn't race the
//! approve handler still mid-flight.

#![cfg(feature = "bus")]

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use acteon_bus::{BusMessage, SharedBackend};
use acteon_core::{BusApproval, BusApprovalEnvelope, BusApprovalStatus, Conversation};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};

/// How long an `Approving` row must have sat unchanged before the
/// reconciler is willing to retry it. Set well past the typical
/// produce latency so a slow request the approve handler is still
/// processing doesn't get a duplicate produce queued behind it.
pub const DEFAULT_RECONCILER_MIN_AGE: Duration = Duration::from_secs(30);

/// How often the sweep runs. Default 60 s — operator-grade
/// timescale; rows are visibly stuck in the admin UI within at most
/// one tick of the produce failing.
pub const DEFAULT_RECONCILER_INTERVAL: Duration = Duration::from_secs(60);

/// Configuration knobs for the reconciler. Tuned by tests; defaults
/// are documented constants above.
#[derive(Debug, Clone)]
pub struct BusReconcilerConfig {
    pub interval: Duration,
    pub min_age: Duration,
}

impl Default for BusReconcilerConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_RECONCILER_INTERVAL,
            min_age: DEFAULT_RECONCILER_MIN_AGE,
        }
    }
}

/// Spawn the bus-approval reconciler on a tokio task. Returns the
/// `JoinHandle` so the caller can keep the task alive for the
/// lifetime of the server (or shut it down explicitly via abort).
pub fn spawn_bus_approval_reconciler(
    state: Arc<dyn StateStore>,
    backend: SharedBackend,
    cfg: &BusReconcilerConfig,
) -> JoinHandle<()> {
    let interval = cfg.interval;
    let min_age = cfg.min_age;
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        // Skip the immediate first tick so we don't sweep at startup
        // before the rest of the server is ready.
        timer.tick().await;
        loop {
            timer.tick().await;
            if let Err(e) = run_once(&*state, &backend, min_age).await {
                warn!(error = %e, "bus approval reconciler tick failed");
            }
        }
    })
}

/// One sweep pass. Public so tests can drive it deterministically
/// without waiting for the timer.
pub async fn run_once(
    state: &dyn StateStore,
    backend: &SharedBackend,
    min_age: Duration,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let entries = state.scan_keys_by_kind(KeyKind::BusApproval).await?;
    let now = Utc::now();
    let min_age_chrono =
        chrono::Duration::from_std(min_age).unwrap_or_else(|_| chrono::Duration::seconds(30));
    let mut retried = 0usize;
    for (key_str, raw) in entries {
        let Ok(approval) = serde_json::from_str::<BusApproval>(&raw) else {
            // Corrupt rows are a real possibility on a long-running
            // system. Skip and let the operator notice via the admin
            // UI rather than crashing the reconciler.
            warn!(key = %key_str, "skipping unparsable bus approval row");
            continue;
        };
        if approval.status != BusApprovalStatus::Approving {
            continue;
        }
        // Don't race the approve handler. Use `decided_at` (set when
        // the row entered Approving) as the age anchor.
        let decided_at = if let Some(t) = approval.decided_at {
            t
        } else {
            warn!(
                approval_id = %approval.approval_id,
                "Approving row has no decided_at; treating as stale and retrying",
            );
            // Without a `decided_at` we can't gauge age. Treat as
            // ancient so the retry runs.
            DateTime::<Utc>::MIN_UTC
        };
        if now - decided_at < min_age_chrono {
            debug!(
                approval_id = %approval.approval_id,
                "skipping recent Approving row",
            );
            continue;
        }
        match retry_approving_row(state, backend, &approval).await {
            Ok(()) => {
                retried += 1;
                info!(
                    approval_id = %approval.approval_id,
                    call_id = %approval.envelope.correlation_token(),
                    "reconciler approved stuck row",
                );
            }
            Err(e) => {
                warn!(
                    approval_id = %approval.approval_id,
                    error = %e,
                    "reconciler retry failed; row remains Approving",
                );
            }
        }
    }
    if retried > 0 {
        info!(retried, "bus approval reconciler tick finished");
    }
    Ok(retried)
}

/// Retry the produce + CAS for a single `Approving` row.
///
/// On the produce step we mirror what the approve handler does:
/// build a `BusMessage` with the standard `acteon.envelope.kind` /
/// `acteon.tool.call_id` / `acteon.correlation_id` / `acteon.reply_to`
/// / `acteon.approval.id` headers, key by `conversation_id`, produce.
/// On the CAS step we transition Approving → Approved and record the
/// produced offset.
async fn retry_approving_row(
    state: &dyn StateStore,
    backend: &SharedBackend,
    approval: &BusApproval,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conv = load_conversation(
        state,
        &approval.namespace,
        &approval.tenant,
        &approval.conversation_id,
    )
    .await?;
    let envelope = match &approval.envelope {
        BusApprovalEnvelope::ToolCall(c) => c.clone(),
    };
    let payload: Value = serde_json::to_value(&envelope)?;
    let events_topic = conv.effective_events_topic();
    let mut msg = BusMessage::new(events_topic.clone(), payload).with_key(&conv.conversation_id);
    msg.headers.insert(
        "acteon.conversation.id".into(),
        conv.conversation_id.clone(),
    );
    if let Some(s) = &envelope.sender {
        msg.headers
            .insert("acteon.conversation.sender".into(), s.clone());
    }
    msg.headers
        .insert("acteon.envelope.kind".into(), "tool_call".into());
    msg.headers
        .insert("acteon.tool.call_id".into(), envelope.call_id.clone());
    if let Some(c) = &envelope.correlation_id {
        msg.headers
            .insert("acteon.correlation_id".into(), c.clone());
    }
    if let Some(r) = &envelope.reply_to {
        msg.headers.insert("acteon.reply_to".into(), r.clone());
    }
    msg.headers
        .insert("acteon.approval.id".into(), approval.approval_id.clone());
    let receipt = backend.produce(msg).await?;
    let key = StateKey::new(
        approval.namespace.clone(),
        approval.tenant.clone(),
        KeyKind::BusApproval,
        &approval.approval_id,
    );
    // CAS Approving → Approved. We re-read the row inside the CAS
    // loop to avoid clobbering a concurrent operator-driven approve
    // that landed between our scan and our CAS.
    for _ in 0..3 {
        let Some((raw, version)) = state.get_versioned(&key).await? else {
            // Row vanished mid-retry. The Kafka record landed; an
            // audit trail with the `acteon.approval.id` header is
            // still recoverable. Log and move on.
            warn!(
                approval_id = %approval.approval_id,
                partition = receipt.partition,
                offset = receipt.offset,
                "approval row deleted between produce and CAS; trace via acteon.approval.id header",
            );
            return Ok(());
        };
        let mut current: BusApproval = serde_json::from_str(&raw)?;
        if current.status != BusApprovalStatus::Approving {
            // Already finalized by the handler or another reconciler
            // pass. Idempotent — nothing to do.
            return Ok(());
        }
        current.status = BusApprovalStatus::Approved;
        current.produced_partition = Some(receipt.partition);
        current.produced_offset = Some(receipt.offset);
        current.produced_at = Some(receipt.timestamp);
        let payload = serde_json::to_string(&current)?;
        if let CasResult::Ok = state
            .compare_and_swap(&key, version, &payload, None)
            .await?
        {
            return Ok(());
        }
        // Conflict — re-read and retry on the next loop iteration.
    }
    // CAS contention exhausted. Message is on Kafka. Next sweep will
    // see the row still Approving and retry; consumer-side dedup
    // keeps the topic clean.
    warn!(
        approval_id = %approval.approval_id,
        "reconciler CAS contention exhausted; row stays Approving for next sweep",
    );
    Ok(())
}

async fn load_conversation(
    state: &dyn StateStore,
    namespace: &str,
    tenant: &str,
    conversation_id: &str,
) -> Result<Conversation, Box<dyn std::error::Error + Send + Sync>> {
    let key = StateKey::new(
        namespace.to_string(),
        tenant.to_string(),
        KeyKind::BusConversation,
        conversation_id,
    );
    let raw = state
        .get(&key)
        .await?
        .ok_or_else(|| format!("conversation {namespace}.{tenant}.{conversation_id} not found"))?;
    Ok(serde_json::from_str(&raw)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use acteon_bus::MemoryBackend;
    use acteon_core::{BusApprovalEnvelope, ToolCall, Topic};
    use acteon_state_memory::MemoryStateStore;

    async fn setup() -> (
        Arc<dyn StateStore>,
        SharedBackend,
        Conversation,
        BusApproval,
    ) {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let backend: SharedBackend = MemoryBackend::new();
        backend
            .create_topic(&Topic::new("conversations-events", "agents", "demo"))
            .await
            .unwrap();
        let mut conv = Conversation::new("thread-1", "agents", "demo");
        conv.participants = vec!["planner-1".into()];
        let conv_key = StateKey::new("agents", "demo", KeyKind::BusConversation, "thread-1");
        state
            .set(&conv_key, &serde_json::to_string(&conv).unwrap(), None)
            .await
            .unwrap();
        let mut call = ToolCall::new("call-1", "billing.refund", serde_json::json!({"usd": 42}));
        call.sender = Some("planner-1".into());
        call.correlation_id = Some("trace-1".into());
        let now = Utc::now();
        // Decided 60 seconds ago — older than the test's min_age.
        let decided_at = now - chrono::Duration::seconds(60);
        let approval = BusApproval {
            approval_id: "appr-1".into(),
            namespace: "agents".into(),
            tenant: "demo".into(),
            conversation_id: conv.conversation_id.clone(),
            reason: Some("paid action".into()),
            envelope: BusApprovalEnvelope::ToolCall(call),
            status: BusApprovalStatus::Approving,
            created_at: now - chrono::Duration::seconds(120),
            expires_at: now + chrono::Duration::hours(1),
            decided_by: Some("ops-1".into()),
            decided_at: Some(decided_at),
            decision_note: Some("verified PO".into()),
            produced_partition: None,
            produced_offset: None,
            produced_at: None,
            labels: HashMap::new(),
        };
        let approval_key = StateKey::new("agents", "demo", KeyKind::BusApproval, "appr-1");
        state
            .set(
                &approval_key,
                &serde_json::to_string(&approval).unwrap(),
                None,
            )
            .await
            .unwrap();
        (state, backend, conv, approval)
    }

    #[tokio::test]
    async fn reconciler_retries_stuck_approving_row() {
        let (state, backend, _conv, _approval) = setup().await;
        let n = run_once(&*state, &backend, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(n, 1, "exactly one row should have been retried");
        // Row must now be Approved with a produced offset.
        let key = StateKey::new("agents", "demo", KeyKind::BusApproval, "appr-1");
        let raw = state.get(&key).await.unwrap().unwrap();
        let final_row: BusApproval = serde_json::from_str(&raw).unwrap();
        assert_eq!(final_row.status, BusApprovalStatus::Approved);
        assert!(final_row.produced_offset.is_some());
        // Decision metadata is preserved (the original `decided_by`
        // sticks; the reconciler doesn't overwrite audit).
        assert_eq!(final_row.decided_by.as_deref(), Some("ops-1"));
    }

    #[tokio::test]
    async fn reconciler_skips_recent_rows() {
        // Decided just now → reconciler should leave it alone for
        // its min_age window.
        let (state, backend, _conv, _approval) = setup().await;
        let key = StateKey::new("agents", "demo", KeyKind::BusApproval, "appr-1");
        let raw = state.get(&key).await.unwrap().unwrap();
        let mut a: BusApproval = serde_json::from_str(&raw).unwrap();
        a.decided_at = Some(Utc::now());
        state
            .set(&key, &serde_json::to_string(&a).unwrap(), None)
            .await
            .unwrap();

        let n = run_once(&*state, &backend, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(n, 0, "recent row should be skipped");
        let raw = state.get(&key).await.unwrap().unwrap();
        let after: BusApproval = serde_json::from_str(&raw).unwrap();
        assert_eq!(after.status, BusApprovalStatus::Approving);
    }

    #[tokio::test]
    async fn reconciler_idempotent_on_already_approved_row() {
        // If the handler raced ahead and already transitioned the row
        // to Approved between our scan and our CAS, the reconciler
        // must not regress it.
        let (state, backend, _conv, _approval) = setup().await;
        let key = StateKey::new("agents", "demo", KeyKind::BusApproval, "appr-1");
        let raw = state.get(&key).await.unwrap().unwrap();
        let mut a: BusApproval = serde_json::from_str(&raw).unwrap();
        a.status = BusApprovalStatus::Approved;
        a.produced_partition = Some(0);
        a.produced_offset = Some(99);
        state
            .set(&key, &serde_json::to_string(&a).unwrap(), None)
            .await
            .unwrap();

        let n = run_once(&*state, &backend, Duration::from_secs(5))
            .await
            .unwrap();
        // The row was already Approved — reconciler skips it.
        assert_eq!(n, 0);
        let raw = state.get(&key).await.unwrap().unwrap();
        let after: BusApproval = serde_json::from_str(&raw).unwrap();
        assert_eq!(after.status, BusApprovalStatus::Approved);
        assert_eq!(after.produced_offset, Some(99));
    }
}
