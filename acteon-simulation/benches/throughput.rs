//! Throughput benchmarks for the Acteon simulation framework.

use std::sync::Arc;

use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_simulation::RecordingProvider;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

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

fn single_dispatch_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let provider = Arc::new(RecordingProvider::new("bench-provider"));
    let gateway = create_gateway(provider.clone());

    c.bench_function("single_dispatch", |b| {
        b.to_async(&rt).iter(|| async {
            let action = test_action();
            gateway.dispatch(action, None).await.unwrap()
        });
    });
}

fn batch_dispatch_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("batch_dispatch");

    for batch_size in [10, 100, 1000] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let gateway = create_gateway(provider.clone());

        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.to_async(&rt).iter(|| async {
                    let actions: Vec<Action> = (0..size).map(|_| test_action()).collect();
                    gateway.dispatch_batch(actions, None).await
                });
            },
        );
    }

    group.finish();
}

fn concurrent_dispatch_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("concurrent_dispatch");

    for concurrency in [10, 50, 100] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let gateway = Arc::new(create_gateway(provider.clone()));

        group.throughput(Throughput::Elements(concurrency as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &num_tasks| {
                b.to_async(&rt).iter(|| {
                    let gateway = Arc::clone(&gateway);
                    async move {
                        let handles: Vec<_> = (0..num_tasks)
                            .map(|_| {
                                let gw = Arc::clone(&gateway);
                                let action = test_action();
                                tokio::spawn(async move { gw.dispatch(action, None).await })
                            })
                            .collect();

                        for handle in handles {
                            handle.await.unwrap().unwrap();
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

fn dispatch_with_rules_benchmark(c: &mut Criterion) {
    use acteon_rules::ir::expr::Expr;
    use acteon_rules::ir::rule::{Rule, RuleAction};

    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("dispatch_with_rules");

    for num_rules in [1, 10, 50] {
        let provider = Arc::new(RecordingProvider::new("bench-provider"));
        let state = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        // Create rules that don't match (to exercise evaluation without side effects)
        let rules: Vec<Rule> = (0..num_rules)
            .map(|i| {
                Rule::new(
                    format!("rule-{i}"),
                    Expr::Binary(
                        acteon_rules::ir::expr::BinaryOp::Eq,
                        Box::new(Expr::Field(
                            Box::new(Expr::Ident("action".into())),
                            "action_type".into(),
                        )),
                        Box::new(Expr::String(format!("nonexistent-{i}"))),
                    ),
                    RuleAction::Suppress,
                )
            })
            .collect();

        let gateway = GatewayBuilder::new()
            .state(state)
            .lock(lock)
            .rules(rules)
            .provider(provider as Arc<dyn DynProvider>)
            .build()
            .expect("gateway should build");

        group.bench_with_input(
            BenchmarkId::from_parameter(num_rules),
            &num_rules,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let action = test_action();
                    gateway.dispatch(action, None).await.unwrap()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    single_dispatch_benchmark,
    batch_dispatch_benchmark,
    concurrent_dispatch_benchmark,
    dispatch_with_rules_benchmark,
);

criterion_main!(benches);
