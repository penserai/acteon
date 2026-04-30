//! Phase 6b streaming-envelope demo.
//!
//! A producer streams a sequence of token chunks for a single
//! `stream_id` and emits a terminal `stream_end`. A consumer scans
//! the events topic, header-filters on the stream id, and stitches
//! the tokens back into a contiguous output until it observes the
//! terminator. Drives the in-memory bus backend directly so no
//! Kafka or HTTP server is required — the production handlers
//! `POST /v1/bus/conversations/.../stream-chunks`, `POST .../stream-end`,
//! and `GET /v1/bus/streams/.../{stream_id}` delegate to the same
//! types and ride the same shared events topic.
//!
//! Scenarios:
//!
//! 1. Producer emits five `StreamChunk` envelopes (`chunk_seq` 0..=4)
//!    plus a terminal `StreamEnd { status: Complete }`.
//! 2. Consumer scans the events topic, header-filters on
//!    `acteon.envelope.kind ∈ {stream_chunk, stream_end}` and
//!    `acteon.stream.id == story-1`, and reassembles the chunks in
//!    order.
//! 3. Stream stops cleanly when `stream_end` is observed.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features bus --example bus_stream_simulation
//! ```

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tracing::{Level, info};

use acteon_bus::{BusMessage, MemoryBackend, ScanFrom};
use acteon_core::{Conversation, StreamChunk, StreamEnd, StreamEndStatus, Topic};

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter("info,acteon_bus=info")
        .init();

    let backend: acteon_bus::SharedBackend = MemoryBackend::new();

    let events = Topic::new("conversations-events", "agents", "demo");
    backend.create_topic(&events).await?;

    let conv = Conversation::new("storytelling-thread", "agents", "demo");
    let topic = conv.effective_events_topic();

    // -----------------------------------------------------------------
    // 1. Producer emits a 5-chunk stream + terminal end
    // -----------------------------------------------------------------

    let stream_id = "story-1";
    let tokens = ["Once ", "upon ", "a ", "time ", "in a far-off land."];

    for (seq, tok) in tokens.iter().enumerate() {
        let mut chunk = StreamChunk::new(
            stream_id,
            i64::try_from(seq).unwrap(),
            json!({"token": tok}),
        );
        chunk.sender = Some("storyteller-1".into());
        chunk.validate()?;

        let payload = serde_json::to_value(&chunk)?;
        let mut msg = BusMessage::new(topic.clone(), payload).with_key(&conv.conversation_id);
        // Conversation-level headers, mirroring the production
        // `post_stream_chunk` handler.
        msg.headers.insert(
            "acteon.conversation.id".into(),
            conv.conversation_id.clone(),
        );
        msg.headers.insert(
            "acteon.conversation.sender".into(),
            chunk.sender.clone().unwrap_or_default(),
        );
        msg.headers
            .insert("acteon.envelope.kind".into(), "stream_chunk".into());
        msg.headers
            .insert("acteon.stream.id".into(), chunk.stream_id.clone());
        msg.headers
            .insert("acteon.stream.seq".into(), chunk.chunk_seq.to_string());
        backend.produce(msg).await?;
        info!(stream_id = %chunk.stream_id, seq = chunk.chunk_seq, "chunk produced");
    }

    let mut end = StreamEnd::complete(stream_id, i64::try_from(tokens.len()).unwrap());
    end.sender = Some("storyteller-1".into());
    end.validate()?;
    let end_payload = serde_json::to_value(&end)?;
    let mut end_msg = BusMessage::new(topic.clone(), end_payload).with_key(&conv.conversation_id);
    end_msg.headers.insert(
        "acteon.conversation.id".into(),
        conv.conversation_id.clone(),
    );
    end_msg.headers.insert(
        "acteon.conversation.sender".into(),
        end.sender.clone().unwrap_or_default(),
    );
    end_msg
        .headers
        .insert("acteon.envelope.kind".into(), "stream_end".into());
    end_msg
        .headers
        .insert("acteon.stream.id".into(), end.stream_id.clone());
    end_msg
        .headers
        .insert("acteon.stream.seq".into(), end.chunk_seq.to_string());
    backend.produce(end_msg).await?;
    info!(stream_id = %end.stream_id, status = ?end.status, "stream_end produced");

    // -----------------------------------------------------------------
    // 2. Consumer reassembles by scanning + header-filtering
    // -----------------------------------------------------------------

    // Same primitive the `consume_stream` SSE handler uses internally:
    // `scan_topic` (Kafka `assign()`, no consumer-group leak), then
    // we filter on `acteon.envelope.kind` and `acteon.stream.id`
    // before deserializing each payload.
    let mut scan = backend.scan_topic(&topic, ScanFrom::Earliest).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut chunks: Vec<StreamChunk> = Vec::new();
    let recovered_end: StreamEnd = loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("timed out reassembling stream".into());
        }
        let remaining = deadline - now;
        tokio::select! {
            next = scan.next() => {
                match next {
                    Some(Ok(msg)) => {
                        let kind = msg
                            .headers
                            .get("acteon.envelope.kind")
                            .map(String::as_str)
                            .unwrap_or_default();
                        let matches = msg
                            .headers
                            .get("acteon.stream.id")
                            .is_some_and(|v| v == stream_id);
                        if !matches {
                            continue;
                        }
                        match kind {
                            "stream_chunk" => {
                                let c: StreamChunk = serde_json::from_value(msg.payload.clone())?;
                                chunks.push(c);
                            }
                            "stream_end" => {
                                let e: StreamEnd = serde_json::from_value(msg.payload.clone())?;
                                break e;
                            }
                            _ => continue,
                        }
                    }
                    Some(Err(_)) | None => return Err("stream ended without terminator".into()),
                }
            }
            () = tokio::time::sleep(remaining) => {
                return Err("scan timeout".into());
            }
        }
    };

    assert_eq!(chunks.len(), tokens.len());
    assert_eq!(recovered_end.status, StreamEndStatus::Complete);
    assert_eq!(recovered_end.stream_id, stream_id);

    // Re-stitch by chunk_seq just like a real consumer would on a
    // partitioned topic where in-partition order is FIFO but cross-
    // partition order is not.
    chunks.sort_by_key(|c| c.chunk_seq);
    let reassembled: String = chunks
        .iter()
        .filter_map(|c| c.body.get("token").and_then(|v| v.as_str()))
        .collect();
    info!(reassembled = %reassembled, chunk_count = chunks.len(), "stream reassembled");
    assert_eq!(reassembled, "Once upon a time in a far-off land.");

    info!("stream simulation complete");
    Ok(())
}
