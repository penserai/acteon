//! Phase 9: agentic bus overhead benchmarks.
//!
//! Measures the cost of typed bus envelopes (tool-calls, streaming
//! chunks, approvals) on the in-memory backend, vs the cost of a
//! raw `BusMessage` publish through the same backend. The delta
//! is what the typed-envelope layer costs in the happy path:
//! payload serialization + header stamping + (for tool-call posts)
//! validation. No Kafka is touched — these are pure
//! Acteon-side-overhead numbers.
//!
//! Why measure against the in-memory backend rather than against
//! Kafka direct? On a real Kafka deployment, network + broker
//! latency dominate by orders of magnitude; the typed-envelope
//! layer is in the noise. The interesting number — and the only
//! one Acteon controls — is what the bus's typed surface costs
//! the producer process *before* the message hits the wire.
//!
//! Run with:
//! ```text
//! cargo bench -p acteon-simulation --features bus --bench bus_overhead
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use acteon_bus::{BusMessage, MemoryBackend, SharedBackend};
use acteon_core::{StreamChunk, StreamEnd, ToolCall, ToolResult, Topic};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use tokio::runtime::Runtime;

const NS: &str = "bench";
const TENANT: &str = "bench";
const TOPIC_NAME: &str = "events";

fn make_backend(rt: &Runtime) -> SharedBackend {
    rt.block_on(async {
        let backend: SharedBackend = MemoryBackend::new();
        backend
            .create_topic(&Topic::new(TOPIC_NAME, NS, TENANT))
            .await
            .expect("create topic");
        backend
    })
}

fn topic_name() -> String {
    format!("{NS}.{TENANT}.{TOPIC_NAME}")
}

/// Baseline: raw `BusMessage` publish with a small JSON payload
/// and no `acteon.*` headers. This is the floor cost of the
/// in-memory backend; everything else measures *additional*
/// overhead.
fn bench_raw_publish(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let backend = make_backend(&rt);
    let topic = topic_name();
    let payload = json!({"k": "v", "n": 42});

    let mut group = c.benchmark_group("bus/raw_publish");
    group.throughput(Throughput::Elements(1));
    group.bench_function("memory_backend", |b| {
        let rt = Arc::clone(&rt);
        let backend = backend.clone();
        let topic = topic.clone();
        let payload = payload.clone();
        b.iter(|| {
            rt.block_on(async {
                let msg = BusMessage::new(topic.clone(), payload.clone()).with_key("conv-1");
                backend.produce(msg).await.unwrap()
            })
        });
    });
    group.finish();
}

/// Phase 6a: post a tool-call envelope. Mirrors the work the
/// production handler does — construct the typed envelope,
/// validate it, serialize to JSON, stamp `acteon.envelope.kind` /
/// `acteon.tool.call_id` / `acteon.correlation_id` headers,
/// produce.
fn bench_tool_call(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let backend = make_backend(&rt);
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/post_tool_call");
    group.throughput(Throughput::Elements(1));
    group.bench_function("memory_backend", |b| {
        let rt = Arc::clone(&rt);
        let backend = backend.clone();
        let topic = topic.clone();
        b.iter(|| {
            rt.block_on(async {
                let mut call =
                    ToolCall::new("call-1", "calendar.list", json!({"day": "2026-04-29"}));
                call.sender = Some("planner-1".into());
                call.correlation_id = Some("trace-1".into());
                call.validate().unwrap();
                let payload = serde_json::to_value(&call).unwrap();
                let mut msg = BusMessage::new(topic.clone(), payload).with_key("conv-1");
                msg.headers
                    .insert("acteon.envelope.kind".into(), "tool_call".into());
                msg.headers
                    .insert("acteon.tool.call_id".into(), call.call_id.clone());
                msg.headers.insert(
                    "acteon.conversation.sender".into(),
                    call.sender.clone().unwrap_or_default(),
                );
                if let Some(c) = &call.correlation_id {
                    msg.headers
                        .insert("acteon.correlation_id".into(), c.clone());
                }
                backend.produce(msg).await.unwrap()
            })
        });
    });
    group.finish();
}

/// Phase 6a: post a tool-result. Same shape as the call but with
/// the `ok` status branch.
fn bench_tool_result(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let backend = make_backend(&rt);
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/post_tool_result");
    group.throughput(Throughput::Elements(1));
    group.bench_function("memory_backend", |b| {
        let rt = Arc::clone(&rt);
        let backend = backend.clone();
        let topic = topic.clone();
        b.iter(|| {
            rt.block_on(async {
                let mut result = ToolResult::ok(
                    "call-1",
                    json!({
                        "events": [{"id": "ev-1", "title": "1:1"}]
                    }),
                );
                result.sender = Some("calendar-svc".into());
                result.validate().unwrap();
                let payload = serde_json::to_value(&result).unwrap();
                let mut msg = BusMessage::new(topic.clone(), payload).with_key("conv-1");
                msg.headers
                    .insert("acteon.envelope.kind".into(), "tool_result".into());
                msg.headers
                    .insert("acteon.tool.call_id".into(), result.call_id.clone());
                backend.produce(msg).await.unwrap()
            })
        });
    });
    group.finish();
}

/// Phase 6b: post a single stream chunk. The hot path for token
/// streaming — every LLM token at production load runs through
/// this code.
fn bench_stream_chunk(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let backend = make_backend(&rt);
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/post_stream_chunk");
    group.throughput(Throughput::Elements(1));
    group.bench_function("memory_backend", |b| {
        let rt = Arc::clone(&rt);
        let backend = backend.clone();
        let topic = topic.clone();
        b.iter(|| {
            rt.block_on(async {
                let mut chunk = StreamChunk::new("stream-1", 0, json!({"token": "Once "}));
                chunk.sender = Some("summarizer".into());
                chunk.validate().unwrap();
                let payload = serde_json::to_value(&chunk).unwrap();
                let mut msg = BusMessage::new(topic.clone(), payload).with_key("conv-1");
                msg.headers
                    .insert("acteon.envelope.kind".into(), "stream_chunk".into());
                msg.headers
                    .insert("acteon.stream.id".into(), chunk.stream_id.clone());
                msg.headers
                    .insert("acteon.stream.seq".into(), chunk.chunk_seq.to_string());
                backend.produce(msg).await.unwrap()
            })
        });
    });
    group.finish();
}

/// Phase 6b: stream-end terminator. Same shape; benchmarked
/// separately so a regression in the cap-on-OK validation path
/// shows up.
fn bench_stream_end(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let backend = make_backend(&rt);
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/post_stream_end");
    group.throughput(Throughput::Elements(1));
    group.bench_function("memory_backend", |b| {
        let rt = Arc::clone(&rt);
        let backend = backend.clone();
        let topic = topic.clone();
        b.iter(|| {
            rt.block_on(async {
                let mut end = StreamEnd::complete("stream-1", 5);
                end.sender = Some("summarizer".into());
                end.validate().unwrap();
                let payload = serde_json::to_value(&end).unwrap();
                let mut msg = BusMessage::new(topic.clone(), payload).with_key("conv-1");
                msg.headers
                    .insert("acteon.envelope.kind".into(), "stream_end".into());
                msg.headers
                    .insert("acteon.stream.id".into(), end.stream_id.clone());
                msg.headers
                    .insert("acteon.stream.seq".into(), end.chunk_seq.to_string());
                backend.produce(msg).await.unwrap()
            })
        });
    });
    group.finish();
}

/// Validation-only (no produce) — isolates the `ToolCall::validate`
/// cost. Useful for catching validation-path regressions independent
/// of backend latency.
fn bench_tool_call_validate(c: &mut Criterion) {
    let mut metadata = HashMap::new();
    metadata.insert("trace".into(), "abc123".into());
    let mut call = ToolCall::new("call-1", "calendar.list", json!({"day": "2026-04-29"}));
    call.sender = Some("planner-1".into());
    call.correlation_id = Some("trace-1".into());
    call.metadata = metadata;

    let mut group = c.benchmark_group("bus/validate");
    group.bench_function("tool_call", |b| {
        b.iter(|| call.validate().unwrap());
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_raw_publish,
    bench_tool_call,
    bench_tool_result,
    bench_stream_chunk,
    bench_stream_end,
    bench_tool_call_validate,
);
criterion_main!(benches);
