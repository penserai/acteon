//! Phase 6c HITL approvals demo.
//!
//! Models the full lifecycle of a pre-publish gate over in-memory
//! state + bus backends — no Kafka or HTTP server required:
//!
//! 1. A requester builds a `ToolCall` and parks it under a
//!    `BusApproval` row (mirrors what the production
//!    `POST /v1/bus/conversations/.../tool-calls` handler does when
//!    `require_approval = true`).
//! 2. The events topic stays empty — nothing has been produced yet.
//! 3. An operator reads the row, attaches a decision note, and
//!    "approves" it by writing the parked envelope onto the events
//!    topic with the standard `acteon.envelope.kind` /
//!    `acteon.tool.call_id` / `acteon.approval.id` audit headers.
//! 4. A second requester parks another tool-call; an operator
//!    rejects this one. The events topic remains empty for that
//!    `call_id`.
//! 5. We scan the events topic and confirm only the approved record
//!    is present.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_approval_simulation
//! ```

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, ScanFrom};
use acteon_core::{
    BusApproval, BusApprovalEnvelope, BusApprovalStatus, Conversation, PauseKind, ToolCall, Topic,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

const NS: &str = "agents";
const TENANT: &str = "demo";
const CONV: &str = "planning-thread";

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();
    let state: std::sync::Arc<dyn StateStore> = std::sync::Arc::new(MemoryStateStore::new());

    let events = Topic::new("conversations-events", NS, TENANT);
    backend.create_topic(&events).await?;
    let conv = Conversation::new(CONV, NS, TENANT);
    let topic = conv.effective_events_topic();

    // -----------------------------------------------------------------
    // 1. Park two tool-calls under separate BusApproval rows.
    // -----------------------------------------------------------------

    let approval_pay = park_tool_call(
        &state,
        &conv,
        ToolCall::new("call-pay-1", "billing.charge", json!({"usd": 42})),
        Some("paid action — operator must approve"),
    )
    .await?;
    info!(approval_id = %approval_pay, "parked paid tool-call (will approve)");

    let approval_export = park_tool_call(
        &state,
        &conv,
        ToolCall::new(
            "call-export-1",
            "users.export",
            json!({"format": "csv", "scope": "all"}),
        ),
        Some("data export — operator must approve"),
    )
    .await?;
    info!(approval_id = %approval_export, "parked export tool-call (will reject)");

    // -----------------------------------------------------------------
    // 2. Verify nothing has been produced yet.
    // -----------------------------------------------------------------

    let pre_count = scan_count_tool_calls(&backend, &topic).await?;
    assert_eq!(
        pre_count, 0,
        "events topic should be empty before approvals decide"
    );

    // -----------------------------------------------------------------
    // 3. Operator approves the first row → produce + transition.
    // -----------------------------------------------------------------

    decide_approval(
        &state,
        &backend,
        &topic,
        &approval_pay,
        Decision::Approve,
        "ops-1",
        Some("verified PO #4711"),
    )
    .await?;
    info!(approval_id = %approval_pay, "approval committed; record produced");

    // -----------------------------------------------------------------
    // 4. Operator rejects the second row → no Kafka record.
    // -----------------------------------------------------------------

    decide_approval(
        &state,
        &backend,
        &topic,
        &approval_export,
        Decision::Reject,
        "ops-1",
        Some("scope too broad"),
    )
    .await?;
    info!(approval_id = %approval_export, "approval rejected; no record produced");

    // -----------------------------------------------------------------
    // 5. Confirm the events topic has exactly one tool-call record,
    //    and it's the approved one. Also confirm the row carries the
    //    expected audit metadata.
    // -----------------------------------------------------------------

    let mut scan = backend.scan_topic(&topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut produced_call_ids: Vec<String> = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        tokio::select! {
            next = scan.next() => {
                match next {
                    Some(Ok(msg)) => {
                        let kind = msg.headers.get("acteon.envelope.kind").map(String::as_str).unwrap_or_default();
                        if kind != "tool_call" { continue; }
                        let cid = msg.headers.get("acteon.tool.call_id").cloned().unwrap_or_default();
                        let approval = msg.headers.get("acteon.approval.id").cloned().unwrap_or_default();
                        info!(call_id = %cid, approval_id = %approval, "produced tool-call observed");
                        produced_call_ids.push(cid);
                    }
                    Some(Err(_)) | None => break,
                }
            }
            () = tokio::time::sleep(deadline - now) => break,
        }
    }
    assert_eq!(produced_call_ids, vec!["call-pay-1".to_string()]);

    let approved: BusApproval = load_approval(&state, &approval_pay).await?;
    assert_eq!(approved.status, BusApprovalStatus::Approved);
    assert_eq!(approved.decided_by.as_deref(), Some("ops-1"));
    assert!(approved.produced_offset.is_some());

    let rejected: BusApproval = load_approval(&state, &approval_export).await?;
    assert_eq!(rejected.status, BusApprovalStatus::Rejected);
    assert!(rejected.produced_offset.is_none());

    info!("approval simulation complete");
    Ok(())
}

async fn park_tool_call(
    state: &std::sync::Arc<dyn StateStore>,
    conv: &Conversation,
    mut call: ToolCall,
    reason: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    call.sender = Some("planner-1".into());
    call.validate()?;
    let approval_id = uuid::Uuid::now_v7().to_string();
    let now = chrono::Utc::now();
    let approval = BusApproval {
        approval_id: approval_id.clone(),
        namespace: conv.namespace.clone(),
        tenant: conv.tenant.clone(),
        kind: PauseKind::OperatorApproval,
        conversation_id: Some(conv.conversation_id.clone()),
        reason: reason.map(str::to_string),
        envelope: Some(BusApprovalEnvelope::ToolCall(call)),
        task_id: None,
        status: BusApprovalStatus::Pending,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        decided_by: None,
        decided_at: None,
        decision_note: None,
        produced_partition: None,
        produced_offset: None,
        produced_at: None,
        labels: Default::default(),
    };
    approval.validate()?;
    let key = StateKey::new(
        conv.namespace.clone(),
        conv.tenant.clone(),
        KeyKind::BusApproval,
        &approval_id,
    );
    state
        .set(&key, &serde_json::to_string(&approval)?, None)
        .await?;
    Ok(approval_id)
}

async fn load_approval(
    state: &std::sync::Arc<dyn StateStore>,
    approval_id: &str,
) -> Result<BusApproval, Box<dyn std::error::Error>> {
    let key = StateKey::new(NS, TENANT, KeyKind::BusApproval, approval_id);
    let raw = state
        .get(&key)
        .await?
        .ok_or_else(|| format!("approval {approval_id} missing"))?;
    Ok(serde_json::from_str(&raw)?)
}

#[derive(Debug, Clone, Copy)]
enum Decision {
    Approve,
    Reject,
}

async fn decide_approval(
    state: &std::sync::Arc<dyn StateStore>,
    backend: &acteon_bus::SharedBackend,
    topic: &str,
    approval_id: &str,
    decision: Decision,
    decided_by: &str,
    decision_note: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut approval = load_approval(state, approval_id).await?;
    if approval.status != BusApprovalStatus::Pending {
        return Err(format!("approval {approval_id} not pending").into());
    }
    let now = chrono::Utc::now();
    match decision {
        Decision::Approve => {
            // Reproduce what the server's `approve_bus_approval`
            // handler does: re-stamp the envelope headers, produce
            // to the events topic with the same key + audit
            // metadata, then transition the row.
            let BusApprovalEnvelope::ToolCall(envelope) = &approval.envelope;
            let payload = serde_json::to_value(envelope)?;
            let mut msg =
                BusMessage::new(topic.to_string(), payload).with_key(&approval.conversation_id);
            msg.headers.insert(
                "acteon.conversation.id".into(),
                approval.conversation_id.clone(),
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
            msg.headers
                .insert("acteon.approval.id".into(), approval.approval_id.clone());
            let receipt = backend.produce(msg).await?;
            approval.status = BusApprovalStatus::Approved;
            approval.produced_partition = Some(receipt.partition);
            approval.produced_offset = Some(receipt.offset);
            approval.produced_at = Some(receipt.timestamp);
        }
        Decision::Reject => {
            approval.status = BusApprovalStatus::Rejected;
        }
    }
    approval.decided_by = Some(decided_by.into());
    approval.decided_at = Some(now);
    approval.decision_note = decision_note.map(str::to_string);
    let key = StateKey::new(NS, TENANT, KeyKind::BusApproval, approval_id);
    state
        .set(&key, &serde_json::to_string(&approval)?, None)
        .await?;
    Ok(())
}

async fn scan_count_tool_calls(
    backend: &acteon_bus::SharedBackend,
    topic: &str,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut scan = backend.scan_topic(topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    let mut count = 0;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(count);
        }
        tokio::select! {
            next = scan.next() => {
                match next {
                    Some(Ok(msg)) => {
                        if msg.headers.get("acteon.envelope.kind").map(String::as_str) == Some("tool_call") {
                            count += 1;
                        }
                    }
                    Some(Err(_)) | None => return Ok(count),
                }
            }
            () = tokio::time::sleep(deadline - now) => return Ok(count),
        }
    }
}
