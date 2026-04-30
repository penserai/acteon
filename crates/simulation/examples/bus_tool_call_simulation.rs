//! Phase 6a tool-call envelope demo.
//!
//! Two agents share an inbox; one calls a tool on the other and
//! retrieves the result by `call_id`. Drives the in-memory bus
//! backend directly so no Kafka or HTTP server is required — the
//! production handlers `POST /v1/bus/conversations/.../tool-calls`,
//! `POST .../tool-results`, and
//! `GET /v1/bus/tool-calls/.../result` delegate to the same types
//! and the same shared events topic.
//!
//! Scenarios:
//!
//! 1. Build a `ToolCall` envelope, wrap it in a conversation message
//!    keyed by `conversation_id`, stamp the standard envelope
//!    headers, and produce.
//! 2. Build a matching `ToolResult` and produce — same conversation,
//!    same correlation token.
//! 3. Scan the events topic, filter on the server-style headers
//!    (`acteon.envelope.kind`, `acteon.tool.call_id`), and recover
//!    the original `ToolResult` payload. The `correlation_id` and
//!    `call_id` propagated through cleanly.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_tool_call_simulation
//! ```

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, ScanFrom};
use acteon_core::{Conversation, ToolCall, ToolResult, ToolResultStatus, Topic};

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();

    // Provision the shared events topic the way agent-registration
    // would (Phase 4) — single tenant, single topic.
    let events = Topic::new("conversations-events", "agents", "demo");
    backend.create_topic(&events).await?;

    let conv = Conversation::new("planning-thread", "agents", "demo");
    let topic = conv.effective_events_topic();

    // -----------------------------------------------------------------
    // 1. Caller produces a ToolCall envelope
    // -----------------------------------------------------------------

    let mut call = ToolCall::new(
        "call-001",
        "calendar.list_events",
        json!({"day": "2026-04-28", "limit": 5}),
    );
    call.sender = Some("planner-1".into());
    call.correlation_id = Some("trace-42".into());
    call.validate()?;

    let payload = serde_json::to_value(&call)?;
    let mut call_msg = BusMessage::new(topic.clone(), payload).with_key(&conv.conversation_id);
    // Server-stamped routing headers — `with_header` strips reserved
    // `acteon.*` keys on user input, so the bus handlers (and this
    // simulation) populate them via direct `headers.insert`.
    call_msg.headers.insert(
        "acteon.conversation.id".into(),
        conv.conversation_id.clone(),
    );
    call_msg.headers.insert(
        "acteon.conversation.sender".into(),
        call.sender.clone().unwrap_or_default(),
    );
    call_msg
        .headers
        .insert("acteon.envelope.kind".into(), "tool_call".into());
    call_msg
        .headers
        .insert("acteon.tool.call_id".into(), call.call_id.clone());
    if let Some(c) = &call.correlation_id {
        call_msg
            .headers
            .insert("acteon.correlation_id".into(), c.clone());
    }
    backend.produce(call_msg).await?;
    info!(
        call_id = %call.call_id,
        tool = %call.tool,
        correlation = %call.correlation_id.clone().unwrap_or_default(),
        "tool-call envelope produced"
    );

    // -----------------------------------------------------------------
    // 2. Responder produces a matching ToolResult envelope
    // -----------------------------------------------------------------

    let mut result = ToolResult::ok(
        &call.call_id,
        json!({
            "events": [
                {"id": "ev-1", "title": "1:1"},
                {"id": "ev-2", "title": "design review"}
            ]
        }),
    );
    result.sender = Some("calendar-svc".into());
    result.correlation_id = call.correlation_id.clone();
    result.validate()?;

    let result_payload = serde_json::to_value(&result)?;
    let mut result_msg =
        BusMessage::new(topic.clone(), result_payload).with_key(&conv.conversation_id);
    result_msg.headers.insert(
        "acteon.conversation.id".into(),
        conv.conversation_id.clone(),
    );
    result_msg.headers.insert(
        "acteon.conversation.sender".into(),
        result.sender.clone().unwrap_or_default(),
    );
    result_msg
        .headers
        .insert("acteon.envelope.kind".into(), "tool_result".into());
    result_msg
        .headers
        .insert("acteon.tool.call_id".into(), result.call_id.clone());
    if let Some(c) = &result.correlation_id {
        result_msg
            .headers
            .insert("acteon.correlation_id".into(), c.clone());
    }
    backend.produce(result_msg).await?;
    info!(
        call_id = %result.call_id,
        status = ?result.status,
        "tool-result envelope produced"
    );

    // -----------------------------------------------------------------
    // 3. Caller scans for the matching result
    // -----------------------------------------------------------------

    // Same primitive the `lookup_tool_result` HTTP handler uses:
    // `scan_topic` reads via Kafka's `assign()` (no consumer-group
    // metadata leak) and we filter on the server-stamped headers
    // before deserializing.
    let mut stream = backend.scan_topic(&topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let recovered: ToolResult = loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timed out waiting for tool result".into());
        }
        let remaining = deadline - now;
        tokio::select! {
            next = stream.next() => {
                match next {
                    Some(Ok(msg)) => {
                        let is_result = msg
                            .headers
                            .get("acteon.envelope.kind")
                            .is_some_and(|v| v == "tool_result");
                        let matches = msg
                            .headers
                            .get("acteon.tool.call_id")
                            .is_some_and(|v| v == &call.call_id);
                        if !is_result || !matches {
                            continue;
                        }
                        break serde_json::from_value(msg.payload.clone())?;
                    }
                    Some(Err(_)) | None => return Err("stream ended without match".into()),
                }
            }
            () = tokio::time::sleep(remaining) => {
                return Err("scan timeout".into());
            }
        }
    };

    assert_eq!(recovered.call_id, call.call_id);
    assert_eq!(recovered.status, ToolResultStatus::Ok);
    assert_eq!(recovered.correlation_id, call.correlation_id);
    info!(
        call_id = %recovered.call_id,
        correlation = %recovered.correlation_id.clone().unwrap_or_default(),
        sender = %recovered.sender.clone().unwrap_or_default(),
        "tool-result recovered by call_id; correlation_id propagated"
    );

    info!("tool-call simulation complete");
    Ok(())
}
