//! Integration tests for pinned-definition garbage collection:
//! `Gateway::gc_pinned_definitions` must delete only `{name}@{version}`
//! entries that no chain state references AND that are older than the
//! registry's `current - 1` for that name.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use acteon_core::chain::{ChainConfig, ChainStepConfig, SignalStepConfig};
use acteon_core::{Action, ActionOutcome, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const NS: &str = "notifications";
const TENANT: &str = "tenant-1";
const PINNED_KIND: &str = "chain_def_pinned";

struct MockProvider;

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &'static str {
        "email"
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn test_chain() -> ChainConfig {
    // Parks on a signal so the execution stays active until we delete it.
    ChainConfig::new("test-chain").with_step(ChainStepConfig::new_wait_for_signal(
        "wait",
        SignalStepConfig {
            signal_name: "go".into(),
            timeout_seconds: None,
            on_timeout: None,
        },
    ))
}

fn build_gateway() -> (Gateway, Arc<dyn StateStore>) {
    let store = Arc::new(MemoryStateStore::new());
    let dyn_store: Arc<dyn StateStore> = store.clone();
    let rule = Rule::new(
        "start-chain",
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("start_chain".into())),
        ),
        RuleAction::Chain {
            chain: "test-chain".into(),
        },
    );
    let gateway = GatewayBuilder::new()
        .state(store)
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(vec![rule])
        .provider(Arc::new(MockProvider))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .chain(test_chain())
        .build()
        .expect("gateway should build");
    (gateway, dyn_store)
}

async fn start_chain(gateway: &Gateway) -> String {
    let action = Action::new(NS, TENANT, "email", "start_chain", serde_json::json!({}));
    match gateway.dispatch(action, None).await.unwrap() {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id,
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

async fn pinned_ids(store: &Arc<dyn StateStore>) -> Vec<String> {
    let mut ids: Vec<String> = store
        .scan_keys(NS, TENANT, KeyKind::Custom(PINNED_KIND.into()), None)
        .await
        .unwrap()
        .into_iter()
        .map(|(key, _)| key.rsplit(':').next().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

async fn delete_chain_state(store: &Arc<dyn StateStore>, chain_id: &str) {
    store
        .delete(&StateKey::new(NS, TENANT, KeyKind::Chain, chain_id))
        .await
        .unwrap();
}

/// Bump the registry definition `times` times.
fn bump_version(gateway: &Gateway, times: u64) {
    for _ in 0..times {
        gateway.set_chain_config(test_chain()).unwrap();
    }
}

#[tokio::test]
async fn gc_keeps_versions_referenced_by_executions() {
    let (gateway, store) = build_gateway();
    start_chain(&gateway).await;
    assert_eq!(pinned_ids(&store).await, vec!["test-chain@1"]);

    // Even far behind the registry, a referenced version survives.
    bump_version(&gateway, 5);
    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 0);
    assert_eq!(pinned_ids(&store).await, vec!["test-chain@1"]);
}

#[tokio::test]
async fn gc_removes_unreferenced_old_versions() {
    let (gateway, store) = build_gateway();
    let chain_id = start_chain(&gateway).await;

    // The execution's state expires (simulated delete) and the
    // definition moves far past v1.
    delete_chain_state(&store, &chain_id).await;
    bump_version(&gateway, 5);

    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 1);
    assert!(pinned_ids(&store).await.is_empty());
}

#[tokio::test]
async fn gc_grace_window_keeps_current_and_previous_versions() {
    let (gateway, store) = build_gateway();
    let chain_id = start_chain(&gateway).await;
    delete_chain_state(&store, &chain_id).await;

    // current = 2: v1 is `current - 1` and stays protected.
    bump_version(&gateway, 1);
    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 0);
    assert_eq!(pinned_ids(&store).await, vec!["test-chain@1"]);

    // current = 3: v1 falls out of the grace window.
    bump_version(&gateway, 1);
    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 1);
    assert!(pinned_ids(&store).await.is_empty());
}

#[tokio::test]
async fn gc_removes_pins_of_deleted_definitions() {
    let (gateway, store) = build_gateway();
    let chain_id = start_chain(&gateway).await;
    delete_chain_state(&store, &chain_id).await;

    // Removing the definition drops the registry protection; with no
    // execution referencing v1 the pin is garbage.
    gateway.remove_chain_config("test-chain").unwrap();
    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 1);
    assert!(pinned_ids(&store).await.is_empty());
}

#[tokio::test]
async fn gc_kept_version_still_resolves_for_the_execution() {
    let (gateway, store) = build_gateway();
    let chain_id = start_chain(&gateway).await;
    bump_version(&gateway, 5);

    assert_eq!(gateway.gc_pinned_definitions().await.unwrap(), 0);

    // The parked execution still advances against its pinned v1.
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.chain_version, 1);
    assert_eq!(
        state.status,
        acteon_core::ChainStatus::WaitingSignal,
        "execution should park on its pinned definition's wait step"
    );
    assert_eq!(pinned_ids(&store).await, vec!["test-chain@1"]);
}
