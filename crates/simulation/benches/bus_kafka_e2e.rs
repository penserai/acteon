//! Phase 10 polish: Kafka-backed end-to-end bench.
//!
//! Companion to `bus_overhead.rs` (which runs against the in-memory
//! backend and isolates Acteon-side overhead). This bench runs the
//! same shapes against `KafkaBackend` to capture wall-clock numbers
//! including broker round-trip, replication, and partition assignment.
//!
//! # Running
//!
//! Requires a reachable Kafka broker. Spin up the workspace's Docker
//! Kafka profile:
//!
//! ```text
//! docker compose -f deploy/docker-compose.yml --profile kafka up -d
//! ```
//!
//! Then point the bench at the broker (defaults to `localhost:9092`):
//!
//! ```text
//! ACTEON_BENCH_KAFKA=localhost:9092 \
//!   cargo bench -p acteon-simulation --features bus --bench bus_kafka_e2e
//! ```
//!
//! When `ACTEON_BENCH_KAFKA` isn't set the bench skips with a clear
//! warning rather than failing — the harness lives in CI but the
//! actual numbers come from a developer with Kafka running locally.
//!
//! # What's measured
//!
//! - Raw publish: wire-time for a single `BusMessage` produce.
//! - Tool-call envelope: full handler-shaped post (typed envelope,
//!   validate, serialize, header stamp, produce).
//! - Stream chunk: hot path for LLM streaming.
//!
//! Compare to `bus_overhead.rs` to see where broker latency
//! dominates Acteon-side overhead. On a local single-broker cluster
//! the typed-envelope layer is in the noise; the broker round-trip
//! is two to three orders of magnitude bigger than the in-memory
//! number.

#![allow(clippy::missing_panics_doc)]

use std::sync::Arc;

use acteon_bus::{BusMessage, KafkaBackend, KafkaBusConfig, SharedBackend};
use acteon_core::{StreamChunk, ToolCall, Topic};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use tokio::runtime::Runtime;

const NS: &str = "bench";
const TENANT: &str = "bench";
const TOPIC_NAME: &str = "kafka-e2e";

fn bootstrap_servers() -> Option<String> {
    std::env::var("ACTEON_BENCH_KAFKA").ok()
}

fn make_kafka_backend(rt: &Runtime, bootstrap: &str) -> Option<SharedBackend> {
    let cfg = KafkaBusConfig {
        bootstrap_servers: bootstrap.to_string(),
        client_id: "acteon-bench".to_string(),
        produce_timeout_ms: 5_000,
        extra: Default::default(),
    };
    let backend = match KafkaBackend::new(&cfg) {
        Ok(b) => b as SharedBackend,
        Err(e) => {
            eprintln!("skipping Kafka e2e bench: backend init failed against {bootstrap}: {e}",);
            return None;
        }
    };
    // Best-effort topic creation. If the broker rejects (already
    // exists, auto-create disabled, etc.) the bench will report
    // produce errors below — also informative.
    let topic = Topic::new(TOPIC_NAME, NS, TENANT);
    rt.block_on(async {
        let _ = backend.create_topic(&topic).await;
    });
    Some(backend)
}

fn topic_name() -> String {
    format!("{NS}.{TENANT}.{TOPIC_NAME}")
}

fn bench_kafka_raw_publish(c: &mut Criterion) {
    let Some(bootstrap) = bootstrap_servers() else {
        eprintln!("ACTEON_BENCH_KAFKA not set; skipping bus/kafka_raw_publish");
        return;
    };
    let rt = Arc::new(Runtime::new().unwrap());
    let Some(backend) = make_kafka_backend(&rt, &bootstrap) else {
        return;
    };
    let topic = topic_name();
    let payload = json!({"k": "v", "n": 42});

    let mut group = c.benchmark_group("bus/kafka_raw_publish");
    group.throughput(Throughput::Elements(1));
    group.bench_function("kafka_backend", |b| {
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

fn bench_kafka_tool_call(c: &mut Criterion) {
    let Some(bootstrap) = bootstrap_servers() else {
        eprintln!("ACTEON_BENCH_KAFKA not set; skipping bus/kafka_post_tool_call");
        return;
    };
    let rt = Arc::new(Runtime::new().unwrap());
    let Some(backend) = make_kafka_backend(&rt, &bootstrap) else {
        return;
    };
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/kafka_post_tool_call");
    group.throughput(Throughput::Elements(1));
    group.bench_function("kafka_backend", |b| {
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

fn bench_kafka_stream_chunk(c: &mut Criterion) {
    let Some(bootstrap) = bootstrap_servers() else {
        eprintln!("ACTEON_BENCH_KAFKA not set; skipping bus/kafka_post_stream_chunk");
        return;
    };
    let rt = Arc::new(Runtime::new().unwrap());
    let Some(backend) = make_kafka_backend(&rt, &bootstrap) else {
        return;
    };
    let topic = topic_name();

    let mut group = c.benchmark_group("bus/kafka_post_stream_chunk");
    group.throughput(Throughput::Elements(1));
    group.bench_function("kafka_backend", |b| {
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

criterion_group!(
    benches,
    bench_kafka_raw_publish,
    bench_kafka_tool_call,
    bench_kafka_stream_chunk,
);
criterion_main!(benches);
