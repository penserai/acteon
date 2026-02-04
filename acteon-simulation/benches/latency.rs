//! Latency benchmarks for the Acteon simulation framework.
//!
//! These benchmarks measure dispatch latency under various load conditions.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_simulation::RecordingProvider;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

fn create_gateway(provider: Arc<dyn DynProvider>) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .provider(provider)
        .build()
        .expect("gateway should build")
}

fn test_action() -> Action {
    Action::new(
        "bench-ns",
        "bench-tenant",
        "bench-provider",
        "bench-action",
        serde_json::json!({"key": "value"}),
    )
}

/// Measure latency percentiles under sustained load.
fn latency_under_load(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("latency_under_load");
    group.measurement_time(Duration::from_secs(10));

    for concurrent_load in [0, 10, 50] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let gateway = Arc::new(create_gateway(provider.clone()));

        group.bench_with_input(
            BenchmarkId::new("concurrent_background", concurrent_load),
            &concurrent_load,
            |b, &load| {
                b.to_async(&rt).iter_custom(|iters| {
                    let gateway = Arc::clone(&gateway);
                    async move {
                        // Start background load
                        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
                        let completed = Arc::new(AtomicU64::new(0));

                        let mut bg_handles = vec![];
                        for _ in 0..load {
                            let gw = Arc::clone(&gateway);
                            let running = Arc::clone(&running);
                            let completed = Arc::clone(&completed);
                            bg_handles.push(tokio::spawn(async move {
                                while running.load(Ordering::Relaxed) {
                                    let action = test_action();
                                    let _ = gw.dispatch(action, None).await;
                                    completed.fetch_add(1, Ordering::Relaxed);
                                    tokio::task::yield_now().await;
                                }
                            }));
                        }

                        // Measure latency for our iterations
                        let start = Instant::now();
                        for _ in 0..iters {
                            let action = test_action();
                            let _ = gateway.dispatch(action, None).await;
                        }
                        let elapsed = start.elapsed();

                        // Stop background load
                        running.store(false, Ordering::Relaxed);
                        for handle in bg_handles {
                            let _ = handle.await;
                        }

                        elapsed
                    }
                });
            },
        );
    }

    group.finish();
}

/// Measure cold vs warm dispatch latency.
fn cold_vs_warm_dispatch(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("cold_vs_warm");

    // Cold dispatch (new gateway each time)
    group.bench_function("cold_dispatch", |b| {
        b.to_async(&rt).iter(|| async {
            let provider = Arc::new(RecordingProvider::new("bench-provider"));
            let gateway = create_gateway(provider);
            let action = test_action();
            gateway.dispatch(action, None).await.unwrap()
        });
    });

    // Warm dispatch (reuse gateway)
    let provider = Arc::new(RecordingProvider::new("bench-provider"));
    let gateway = create_gateway(provider.clone());

    // Warm up the gateway
    rt.block_on(async {
        for _ in 0..100 {
            let action = test_action();
            gateway.dispatch(action, None).await.unwrap();
        }
    });

    group.bench_function("warm_dispatch", |b| {
        b.to_async(&rt).iter(|| async {
            let action = test_action();
            gateway.dispatch(action, None).await.unwrap()
        });
    });

    group.finish();
}

/// Measure latency with different payload sizes.
fn payload_size_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("payload_size");

    for payload_size in [100, 1000, 10000] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let gateway = create_gateway(provider.clone());

        // Create a payload of approximately the target size
        let payload_data: String = "x".repeat(payload_size);

        group.bench_with_input(
            BenchmarkId::from_parameter(payload_size),
            &payload_data,
            |b, data| {
                b.to_async(&rt).iter(|| async {
                    let action = Action::new(
                        "bench-ns",
                        "bench-tenant",
                        "bench-provider",
                        "bench-action",
                        serde_json::json!({"data": data}),
                    );
                    gateway.dispatch(action, None).await.unwrap()
                });
            },
        );
    }

    group.finish();
}

/// Measure lock contention impact on latency.
fn lock_contention_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("lock_contention");
    group.measurement_time(Duration::from_secs(5));

    for contention_level in [1, 5, 10] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let gateway = Arc::new(create_gateway(provider.clone()));

        group.bench_with_input(
            BenchmarkId::new("same_action_id", contention_level),
            &contention_level,
            |b, &level| {
                b.to_async(&rt).iter_custom(|iters| {
                    let gateway = Arc::clone(&gateway);
                    async move {
                        let start = Instant::now();

                        // Create multiple tasks trying to dispatch simultaneously
                        let handles: Vec<_> = (0..level)
                            .map(|_| {
                                let gw = Arc::clone(&gateway);
                                tokio::spawn(async move {
                                    for _ in 0..(iters / level as u64).max(1) {
                                        let action = test_action();
                                        let _ = gw.dispatch(action, None).await;
                                    }
                                })
                            })
                            .collect();

                        for handle in handles {
                            handle.await.unwrap();
                        }

                        start.elapsed()
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    latency_under_load,
    cold_vs_warm_dispatch,
    payload_size_latency,
    lock_contention_latency,
);

criterion_main!(benches);
