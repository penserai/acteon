//! A2A Task-lifecycle benchmark (Phase 5 — load test).
//!
//! Measures the `TaskEngine` hot path that an A2A `message/send` /
//! `tasks/*` request drives:
//!
//! - individual mutations (`create_task`, `transition_task`,
//!   `append_history`, `apply_artifact_update`);
//! - the full lifecycle `create → Working → append → artifact →
//!   Completed`, with and without a stream broadcast attached;
//! - the same lifecycle with **N concurrent SSE subscribers**
//!   registered on the broadcast, so the cost of subscriber count
//!   on the emit path is visible;
//! - the stale-task reaper (`fail_if_stale`).
//!
//! Each `TaskEngine` op is a CAS-retried state-store round trip
//! against the in-memory backend, so the numbers are a clean
//! measure of the engine's own overhead — no network, no disk.
//!
//! Run with: `cargo bench -p acteon-gateway --bench a2a_task_lifecycle`

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Duration as ChronoDuration, Utc};
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use acteon_core::{
    Artifact, StreamEvent, Task, TaskArtifactUpdateEvent, TaskMessage, TaskPart, TaskRole,
    TaskState,
};
use acteon_gateway::{TaskEngine, TaskScope};
use acteon_state_memory::MemoryStateStore;

const NS: &str = "agents";
const TENANT: &str = "demo";

/// Monotonic id source so every `create_task` in a bench gets a
/// fresh, never-before-used task id (a duplicate would error
/// `AlreadyExists` and skew the measurement).
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_id() -> String {
    format!("bench-task-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn scope() -> TaskScope {
    TaskScope::new(NS, TENANT)
}

fn seed_task(id: &str) -> Task {
    Task::new(id, NS, TENANT)
}

fn agent_msg(message_id: &str, task_id: &str, text: &str) -> TaskMessage {
    let mut m = TaskMessage::text(message_id.to_string(), TaskRole::Agent, text);
    m.task_id = Some(task_id.to_string());
    m
}

/// `create_task` — one CAS-checked insert.
fn bench_create_task(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()));
    let scope = scope();
    c.bench_function("a2a_create_task", |b| {
        b.iter(|| {
            rt.block_on(async {
                let id = next_id();
                let task = engine.create_task(seed_task(&id)).await.expect("create");
                let _ = black_box(&task);
                // Read it back so the bench covers the round trip a
                // `tasks/get` would also pay.
                let got = engine.get_task(&scope, &id).await.expect("get");
                black_box(got)
            })
        });
    });
}

/// `transition_task` — one CAS read-modify-write. Uses
/// `iter_batched` so each timed iteration transitions a *fresh*
/// Submitted task (transitioning a terminal task would error).
fn bench_transition(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()));
    let scope = scope();
    c.bench_function("a2a_transition_submitted_to_working", |b| {
        b.iter_batched(
            || {
                // Setup (untimed): mint a fresh Submitted task.
                let id = next_id();
                rt.block_on(async {
                    engine.create_task(seed_task(&id)).await.expect("create");
                });
                id
            },
            |id| {
                // Timed: the single transition.
                rt.block_on(async {
                    let t = engine
                        .transition_task(&scope, &id, TaskState::Working, None)
                        .await
                        .expect("transition");
                    black_box(t)
                })
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

/// Drive one task through the whole reachable lifecycle. Optionally
/// attaches a stream broadcast so the emit-path cost is included.
async fn run_lifecycle(engine: &TaskEngine, scope: &TaskScope) {
    let id = next_id();
    engine.create_task(seed_task(&id)).await.expect("create");
    engine
        .transition_task(scope, &id, TaskState::Working, None)
        .await
        .expect("→working");
    engine
        .append_history(
            scope,
            &id,
            agent_msg(&format!("{id}-m"), &id, "working on it"),
        )
        .await
        .expect("append");
    engine
        .apply_artifact_update(
            scope,
            TaskArtifactUpdateEvent::single_shot(
                &id,
                Artifact::new(format!("{id}-art"), vec![TaskPart::text("output")]),
            ),
        )
        .await
        .expect("artifact");
    engine
        .transition_task(scope, &id, TaskState::Completed, None)
        .await
        .expect("→completed");
}

/// Full lifecycle with no stream broadcast — the baseline.
fn bench_lifecycle_no_stream(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()));
    let scope = scope();
    c.bench_function("a2a_full_lifecycle_no_stream", |b| {
        b.iter(|| rt.block_on(run_lifecycle(&engine, &scope)));
    });
}

/// Full lifecycle with a stream broadcast attached + N subscribers
/// registered. Parametrized over N so the cost of subscriber count
/// on the emit path is visible — broadcast `send` is O(1) in
/// receiver count for the send itself, but the per-receiver slab
/// grows, so this confirms the emit path doesn't degrade.
fn bench_lifecycle_streamed(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut group = c.benchmark_group("a2a_full_lifecycle_streamed");
    for subscribers in [0_usize, 1, 8, 64] {
        let (tx, _root) = tokio::sync::broadcast::channel::<StreamEvent>(1024);
        let engine = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx.clone());
        let scope = scope();
        // Hold N receivers for the whole bench. They are not drained
        // — the 1024-slot channel absorbs the ~5 events per
        // lifecycle without lagging, so the bench measures the
        // send-side cost with N receivers registered.
        let _receivers: Vec<_> = (0..subscribers).map(|_| tx.subscribe()).collect();
        group.bench_with_input(
            BenchmarkId::from_parameter(subscribers),
            &subscribers,
            |b, _| {
                b.iter(|| rt.block_on(run_lifecycle(&engine, &scope)));
            },
        );
    }
    group.finish();
}

/// Full lifecycle with N subscribers that are *actively drained* by
/// spawned tasks — the realistic end-to-end SSE-consumer load. This
/// is the closest the bench gets to "streamed Task lifecycle under
/// N concurrent subscribers" from the design doc.
fn bench_lifecycle_drained_subscribers(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("multi-thread runtime");
    let mut group = c.benchmark_group("a2a_lifecycle_drained_subscribers");
    for subscribers in [1_usize, 8, 64] {
        let (tx, _root) = tokio::sync::broadcast::channel::<StreamEvent>(1024);
        let engine = TaskEngine::new(Arc::new(MemoryStateStore::new())).with_stream_tx(tx.clone());
        let scope = scope();
        // Spawn N draining tasks. Each pulls events until the
        // channel closes; for the bench they just keep the receive
        // side hot so `send` contends with real consumers.
        for _ in 0..subscribers {
            let mut rx = tx.subscribe();
            rt.spawn(async move {
                while rx.recv().await.is_ok() {
                    // Touch the event so the compiler can't elide
                    // the receive.
                    std::hint::black_box(());
                }
            });
        }
        group.bench_with_input(
            BenchmarkId::from_parameter(subscribers),
            &subscribers,
            |b, _| {
                b.iter(|| rt.block_on(run_lifecycle(&engine, &scope)));
            },
        );
    }
    group.finish();
}

/// Stale-task reaper. `iter_batched` mints a fresh Working task per
/// timed iteration; the reap is evaluated against a `now` an hour
/// in the future so the task is unambiguously stale.
fn bench_stale_reaper(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let engine = TaskEngine::new(Arc::new(MemoryStateStore::new()));
    let scope = scope();
    c.bench_function("a2a_stale_reaper_fail_if_stale", |b| {
        b.iter_batched(
            || {
                let id = next_id();
                rt.block_on(async {
                    engine.create_task(seed_task(&id)).await.expect("create");
                    engine
                        .transition_task(&scope, &id, TaskState::Working, None)
                        .await
                        .expect("→working");
                });
                id
            },
            |id| {
                rt.block_on(async {
                    let future = Utc::now() + ChronoDuration::hours(1);
                    let reaped = engine
                        .fail_if_stale(&scope, &id, future)
                        .await
                        .expect("reap call");
                    black_box(reaped)
                })
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_create_task,
    bench_transition,
    bench_lifecycle_no_stream,
    bench_lifecycle_streamed,
    bench_lifecycle_drained_subscribers,
    bench_stale_reaper,
);
criterion_main!(benches);
