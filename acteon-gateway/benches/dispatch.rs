use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

use acteon_core::{Action, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

struct BenchProvider {
    provider_name: String,
}

impl BenchProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_owned(),
        }
    }
}

#[async_trait]
impl DynProvider for BenchProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn test_action() -> Action {
    Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Hello",
            "priority": 5
        }),
    )
}

fn executor_config() -> ExecutorConfig {
    ExecutorConfig {
        max_retries: 0,
        execution_timeout: Duration::from_secs(5),
        max_concurrent: 100,
        ..ExecutorConfig::default()
    }
}

fn bench_dispatch_no_rules(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let gateway = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .provider(Arc::new(BenchProvider::new("email")))
        .executor_config(executor_config())
        .build()
        .expect("gateway should build");

    c.bench_function("dispatch_no_rules", |b| {
        b.iter(|| {
            let action = test_action();
            rt.block_on(async {
                let result = gateway.dispatch(black_box(action), None).await;
                black_box(result)
            })
        });
    });
}

fn bench_dispatch_with_rules(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let yaml = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
  - name: dedup-email
    priority: 2
    condition:
      all:
        - field: action.action_type
          eq: "send_sms"
        - field: action.payload.to
          contains: "+1"
    action:
      type: deduplicate
      ttl_seconds: 300
  - name: throttle-high-volume
    priority: 3
    condition:
      field: action.payload.priority
      lt: 2
    action:
      type: throttle
      max_count: 1000
      window_seconds: 60
  - name: reroute-urgent
    priority: 4
    condition:
      field: action.payload.priority
      gt: 9
    action:
      type: reroute
      target_provider: "sms-fallback"
  - name: allow-rest
    priority: 100
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: allow
"#;

    let frontend = YamlFrontend;
    let rules = frontend.parse(yaml).expect("YAML rules should parse");

    let gateway = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(rules)
        .provider(Arc::new(BenchProvider::new("email")))
        .provider(Arc::new(BenchProvider::new("sms-fallback")))
        .executor_config(executor_config())
        .build()
        .expect("gateway should build");

    c.bench_function("dispatch_with_5_rules", |b| {
        b.iter(|| {
            let action = test_action();
            rt.block_on(async {
                let result = gateway.dispatch(black_box(action), None).await;
                black_box(result)
            })
        });
    });
}

fn bench_dispatch_batch(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let rules = vec![
        Rule::new(
            "check-type",
            Expr::Binary(
                BinaryOp::Eq,
                Box::new(Expr::Field(
                    Box::new(Expr::Ident("action".into())),
                    "action_type".into(),
                )),
                Box::new(Expr::String("spam".into())),
            ),
            RuleAction::Suppress,
        )
        .with_priority(1),
        Rule::new("allow-all", Expr::Bool(true), RuleAction::Allow).with_priority(100),
    ];

    let gateway = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(rules)
        .provider(Arc::new(BenchProvider::new("email")))
        .executor_config(executor_config())
        .build()
        .expect("gateway should build");

    c.bench_function("dispatch_batch_10_actions", |b| {
        b.iter(|| {
            let actions: Vec<Action> = (0..10).map(|_| test_action()).collect();
            rt.block_on(async {
                let results = gateway.dispatch_batch(black_box(actions), None).await;
                black_box(results)
            })
        });
    });
}

criterion_group!(
    benches,
    bench_dispatch_no_rules,
    bench_dispatch_with_rules,
    bench_dispatch_batch,
);
criterion_main!(benches);
