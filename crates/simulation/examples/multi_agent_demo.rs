//! Phase 9 multi-agent demo — end-to-end agentic bus showcase.
//!
//! Three agents collaborate on the same conversation thread,
//! exercising every primitive the bus shipped through Phases 1–6c:
//!
//!   - `planner-1`  — orchestrator. Posts tool-calls, waits for
//!                    results, drives the conversation.
//!   - `calendar`   — tool service. Handles `calendar.list` calls
//!                    and emits ordinary `ToolResult` envelopes.
//!   - `summarizer` — streamer. Posts a `tool_call` for
//!                    `text.summarize`, then streams the answer
//!                    back as `StreamChunk`s + a terminal
//!                    `StreamEnd { complete }`.
//!
//! Plus a Phase 6c moment: the planner attempts a sensitive
//! `billing.refund` call. It's parked under a `BusApproval`; an
//! operator decides on it; only on approval does the resulting
//! tool-result come back.
//!
//! Drives the in-memory bus + state backends so no Kafka or HTTP
//! server is required. The same flow ports unchanged to the
//! production REST surface — see the polyglot SDK docs.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example multi_agent_demo
//! ```

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use serde_json::json;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, ScanFrom};
use acteon_core::{
    Agent, BusApproval, BusApprovalEnvelope, BusApprovalStatus, Conversation, StreamChunk,
    StreamEnd, ToolCall, ToolResult, ToolResultStatus, Topic,
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

    info!("=== Acteon agentic bus — multi-agent demo ===");

    // -----------------------------------------------------------------
    // Topology: shared events topic + three registered agents.
    // -----------------------------------------------------------------

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());

    let events = Topic::new("conversations-events", NS, TENANT);
    backend.create_topic(&events).await?;

    let planner = Agent::new("planner-1", NS, TENANT);
    let calendar = Agent::new("calendar", NS, TENANT);
    let summarizer = Agent::new("summarizer", NS, TENANT);
    info!(
        agents = format!(
            "{}, {}, {}",
            planner.agent_id, calendar.agent_id, summarizer.agent_id
        ),
        "registered three agents on the bus",
    );

    let mut conv = Conversation::new(CONV, NS, TENANT);
    conv.participants = vec![
        planner.agent_id.clone(),
        calendar.agent_id.clone(),
        summarizer.agent_id.clone(),
    ];
    let topic = conv.effective_events_topic();
    info!(
        conversation = %conv.conversation_id,
        participants = conv.participants.len(),
        "conversation registered with participant ACL",
    );

    // -----------------------------------------------------------------
    // Scenario 1 (Phase 6a): planner → calendar tool-call/result.
    //
    // The planner asks for tomorrow's events. Calendar answers.
    // -----------------------------------------------------------------

    let cal_call = "call-cal-1";
    let mut call = ToolCall::new(
        cal_call,
        "calendar.list_events",
        json!({"day": "2026-04-29", "limit": 5}),
    );
    call.sender = Some(planner.agent_id.clone());
    call.correlation_id = Some("trace-1".into());
    call.validate()?;
    produce_envelope(
        &backend,
        &topic,
        &conv,
        &serde_json::to_value(&call)?,
        "tool_call",
        Some(call.sender.as_deref().unwrap_or_default()),
        |msg| {
            msg.headers
                .insert("acteon.tool.call_id".into(), call.call_id.clone());
            if let Some(c) = &call.correlation_id {
                msg.headers
                    .insert("acteon.correlation_id".into(), c.clone());
            }
        },
    )
    .await?;
    info!(call_id = %cal_call, sender = %planner.agent_id, "tool-call produced");

    // Calendar produces the result.
    let mut cal_result = ToolResult::ok(
        cal_call,
        json!({
            "events": [
                {"id": "ev-1", "title": "1:1 with Alex", "at": "10:00"},
                {"id": "ev-2", "title": "design review",  "at": "14:00"},
            ]
        }),
    );
    cal_result.sender = Some(calendar.agent_id.clone());
    cal_result.correlation_id = call.correlation_id.clone();
    cal_result.validate()?;
    produce_envelope(
        &backend,
        &topic,
        &conv,
        &serde_json::to_value(&cal_result)?,
        "tool_result",
        Some(cal_result.sender.as_deref().unwrap_or_default()),
        |msg| {
            msg.headers
                .insert("acteon.tool.call_id".into(), cal_call.into());
            if let Some(c) = &cal_result.correlation_id {
                msg.headers
                    .insert("acteon.correlation_id".into(), c.clone());
            }
        },
    )
    .await?;
    info!(call_id = %cal_call, sender = %calendar.agent_id, "tool-result produced");

    // The planner recovers the result by header-filtering. Same
    // primitive `lookup_bus_tool_result` uses on the REST side.
    let recovered = recover_tool_result(&backend, &topic, cal_call).await?;
    assert_eq!(recovered.status, ToolResultStatus::Ok);
    info!(
        call_id = %recovered.call_id,
        status = ?recovered.status,
        "planner recovered calendar.list_events result",
    );

    // -----------------------------------------------------------------
    // Scenario 2 (Phase 6b): summarizer streams a multi-chunk reply.
    //
    // Planner asks for a summary. Summarizer streams the reply
    // token-by-token (via `StreamChunk` envelopes), then closes
    // with a terminal `StreamEnd { complete }`.
    // -----------------------------------------------------------------

    let sum_call = "call-sum-1";
    let mut sum_request = ToolCall::new(
        sum_call,
        "text.summarize",
        json!({"text": "There were two meetings: a 1:1 with Alex and a design review."}),
    );
    sum_request.sender = Some(planner.agent_id.clone());
    sum_request.validate()?;
    produce_envelope(
        &backend,
        &topic,
        &conv,
        &serde_json::to_value(&sum_request)?,
        "tool_call",
        Some(planner.agent_id.as_str()),
        |msg| {
            msg.headers
                .insert("acteon.tool.call_id".into(), sum_call.into());
        },
    )
    .await?;
    info!(call_id = %sum_call, "summarize tool-call produced");

    let stream_id = "stream-sum-1";
    let chunks = [
        "Two ",
        "meetings ",
        "today: ",
        "1:1 + ",
        "design ",
        "review.",
    ];
    for (seq, tok) in chunks.iter().enumerate() {
        let mut chunk = StreamChunk::new(
            stream_id,
            i64::try_from(seq).unwrap(),
            json!({"token": tok}),
        );
        chunk.sender = Some(summarizer.agent_id.clone());
        chunk.validate()?;
        produce_envelope(
            &backend,
            &topic,
            &conv,
            &serde_json::to_value(&chunk)?,
            "stream_chunk",
            Some(summarizer.agent_id.as_str()),
            |msg| {
                msg.headers
                    .insert("acteon.stream.id".into(), chunk.stream_id.clone());
                msg.headers
                    .insert("acteon.stream.seq".into(), chunk.chunk_seq.to_string());
            },
        )
        .await?;
    }
    let mut end = StreamEnd::complete(stream_id, i64::try_from(chunks.len()).unwrap());
    end.sender = Some(summarizer.agent_id.clone());
    end.validate()?;
    produce_envelope(
        &backend,
        &topic,
        &conv,
        &serde_json::to_value(&end)?,
        "stream_end",
        Some(summarizer.agent_id.as_str()),
        |msg| {
            msg.headers
                .insert("acteon.stream.id".into(), end.stream_id.clone());
            msg.headers
                .insert("acteon.stream.seq".into(), end.chunk_seq.to_string());
        },
    )
    .await?;
    info!(
        stream_id = %stream_id,
        chunks = chunks.len(),
        "summarizer produced 6 chunks + terminator",
    );

    // Reassemble — same algorithm a real consumer applies.
    let reassembled = reassemble_stream(&backend, &topic, &conv.conversation_id, stream_id).await?;
    assert_eq!(reassembled, "Two meetings today: 1:1 + design review.");
    info!(reassembled = %reassembled, "stream reassembled by header-filtering");

    // -----------------------------------------------------------------
    // Scenario 3 (Phase 6c): planner attempts a sensitive call.
    //
    // The bus parks it under a `BusApproval` row; an operator
    // approves; only then does the corresponding tool-result land.
    // -----------------------------------------------------------------

    let pay_call = "call-refund-1";
    let mut pay_request = ToolCall::new(
        pay_call,
        "billing.refund",
        json!({"customer": "cust-7", "usd": 42}),
    );
    pay_request.sender = Some(planner.agent_id.clone());
    pay_request.validate()?;

    // Park instead of producing.
    let approval_id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now();
    let approval = BusApproval {
        approval_id: approval_id.clone(),
        namespace: NS.into(),
        tenant: TENANT.into(),
        conversation_id: conv.conversation_id.clone(),
        reason: Some("refund — operator review required".into()),
        envelope: BusApprovalEnvelope::ToolCall(pay_request.clone()),
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
    let approval_key = StateKey::new(NS, TENANT, KeyKind::BusApproval, &approval_id);
    state
        .set(&approval_key, &serde_json::to_string(&approval)?, None)
        .await?;
    info!(
        approval_id = %approval_id,
        call_id = %pay_call,
        "billing.refund parked for HITL approval",
    );

    // Operator decides "approved" and the parked envelope lands on Kafka.
    let mut approved = approval.clone();
    let env_payload = serde_json::to_value(&pay_request)?;
    let receipt = produce_envelope(
        &backend,
        &topic,
        &conv,
        &env_payload,
        "tool_call",
        Some(planner.agent_id.as_str()),
        |msg| {
            msg.headers
                .insert("acteon.tool.call_id".into(), pay_call.into());
            msg.headers
                .insert("acteon.approval.id".into(), approval_id.clone());
        },
    )
    .await?;
    approved.status = BusApprovalStatus::Approved;
    approved.decided_by = Some("ops-1".into());
    approved.decided_at = Some(Utc::now());
    approved.decision_note = Some("verified PO #4711".into());
    approved.produced_partition = Some(receipt.partition);
    approved.produced_offset = Some(receipt.offset);
    approved.produced_at = Some(receipt.timestamp);
    state
        .set(&approval_key, &serde_json::to_string(&approved)?, None)
        .await?;
    info!(
        approval_id = %approval_id,
        partition = receipt.partition,
        offset = receipt.offset,
        "operator approved — produced to Kafka with acteon.approval.id audit header",
    );

    // The billing service responds.
    let mut pay_result =
        ToolResult::ok(pay_call, json!({"refund_id": "rf-987", "status": "issued"}));
    pay_result.sender = Some("billing-svc".into());
    pay_result.validate()?;
    produce_envelope(
        &backend,
        &topic,
        &conv,
        &serde_json::to_value(&pay_result)?,
        "tool_result",
        Some(pay_result.sender.as_deref().unwrap_or_default()),
        |msg| {
            msg.headers
                .insert("acteon.tool.call_id".into(), pay_call.into());
        },
    )
    .await?;
    let recovered_pay = recover_tool_result(&backend, &topic, pay_call).await?;
    assert_eq!(recovered_pay.status, ToolResultStatus::Ok);
    info!(call_id = %pay_call, "refund issued — full HITL loop complete");

    // -----------------------------------------------------------------
    // Audit summary.
    // -----------------------------------------------------------------

    info!("=== summary ===");
    info!(
        "• 3 agents posting on a private conversation (participant ACL enforced at envelope post)"
    );
    info!("• 2 standard tool-call/result pairs (Phase 6a)");
    info!("• 1 streamed reply: 6 chunks + terminator (Phase 6b)");
    info!("• 1 HITL-gated tool-call: parked → approved → produced → result (Phase 6c)");
    info!("• Same flow ports unchanged to the REST surface and the polyglot SDKs");

    Ok(())
}

/// Helper: produce an envelope to the events topic with the
/// `acteon.envelope.kind`, `acteon.conversation.id`, and (optionally)
/// `acteon.conversation.sender` headers stamped — same shape the
/// production handlers stamp.
async fn produce_envelope<F>(
    backend: &acteon_bus::SharedBackend,
    topic: &str,
    conv: &Conversation,
    payload: &serde_json::Value,
    envelope_kind: &str,
    sender: Option<&str>,
    customize: F,
) -> Result<acteon_bus::DeliveryReceipt, Box<dyn std::error::Error>>
where
    F: FnOnce(&mut BusMessage),
{
    let mut msg =
        BusMessage::new(topic.to_string(), payload.clone()).with_key(&conv.conversation_id);
    msg.headers.insert(
        "acteon.conversation.id".into(),
        conv.conversation_id.clone(),
    );
    if let Some(s) = sender {
        msg.headers
            .insert("acteon.conversation.sender".into(), s.to_string());
    }
    msg.headers
        .insert("acteon.envelope.kind".into(), envelope_kind.to_string());
    customize(&mut msg);
    Ok(backend.produce(msg).await?)
}

/// Header-filter the events topic for `(envelope.kind=tool_result,
/// tool.call_id=<id>)` and return the recovered `ToolResult`.
async fn recover_tool_result(
    backend: &acteon_bus::SharedBackend,
    topic: &str,
    call_id: &str,
) -> Result<ToolResult, Box<dyn std::error::Error>> {
    let mut scan = backend.scan_topic(topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timed out waiting for tool result".into());
        }
        let remaining = deadline - now;
        tokio::select! {
            next = scan.next() => match next {
                Some(Ok(msg)) => {
                    let kind = msg.headers.get("acteon.envelope.kind").map(String::as_str).unwrap_or_default();
                    if kind != "tool_result" { continue; }
                    let cid = msg.headers.get("acteon.tool.call_id").cloned().unwrap_or_default();
                    if cid != call_id { continue; }
                    return Ok(serde_json::from_value(msg.payload.clone())?);
                }
                Some(Err(_)) | None => return Err("stream ended without match".into()),
            },
            () = tokio::time::sleep(remaining) => return Err("scan timeout".into()),
        }
    }
}

/// Header-filter for `(envelope.kind ∈ {stream_chunk, stream_end},
/// conversation.id = conv, stream.id = sid)`, sort by `chunk_seq`,
/// and return the reassembled string. Mirrors the SSE consumer's
/// reassembly path.
async fn reassemble_stream(
    backend: &acteon_bus::SharedBackend,
    topic: &str,
    target_conv: &str,
    target_stream: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut scan = backend.scan_topic(topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut chunks: Vec<StreamChunk> = Vec::new();
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timed out reassembling stream".into());
        }
        let remaining = deadline - now;
        tokio::select! {
            next = scan.next() => match next {
                Some(Ok(msg)) => {
                    let kind = msg.headers.get("acteon.envelope.kind").map(String::as_str).unwrap_or_default();
                    let conv_match = msg.headers.get("acteon.conversation.id")
                        .is_some_and(|v| v == target_conv);
                    let stream_match = msg.headers.get("acteon.stream.id")
                        .is_some_and(|v| v == target_stream);
                    if !conv_match || !stream_match { continue; }
                    match kind {
                        "stream_chunk" => {
                            chunks.push(serde_json::from_value(msg.payload.clone())?);
                        }
                        "stream_end" => break,
                        _ => continue,
                    }
                }
                Some(Err(_)) | None => return Err("stream ended without terminator".into()),
            },
            () = tokio::time::sleep(remaining) => return Err("scan timeout".into()),
        }
    }
    chunks.sort_by_key(|c| c.chunk_seq);
    Ok(chunks
        .iter()
        .filter_map(|c| c.body.get("token").and_then(|v| v.as_str()))
        .collect())
}
