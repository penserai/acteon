//! Fault-injection coverage for terminal chain history recovery.

use std::sync::Arc;

use acteon_core::{
    Action, ActionOutcome, ExecutionEventType,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state::{
    KeyKind,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use serde_json::json;

const NAMESPACE: &str = "ns";
const TENANT: &str = "tenant";

struct Fixture {
    fault: Arc<FaultStore>,
    gateway: Gateway,
}

impl Fixture {
    fn new() -> Self {
        let state = Arc::new(MemoryStateStore::new());
        let fault = Arc::new(FaultStore::new(state));
        let chain = ChainConfig::new("flow").with_step(ChainStepConfig::new_timer(
            "wait",
            TimerStepConfig {
                duration_seconds: Some(60),
                until: None,
            },
        ));
        let gateway = GatewayBuilder::new()
            .state(fault.clone())
            .lock(Arc::new(MemoryDistributedLock::new()))
            .chain(chain)
            .rules(vec![Rule::new(
                "start",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: "flow".to_owned(),
                },
            )])
            .build()
            .unwrap();
        Self { fault, gateway }
    }

    async fn start(&self) -> String {
        let outcome = self
            .gateway
            .dispatch(
                Action::new(NAMESPACE, TENANT, "source", "start", json!({})),
                None,
            )
            .await
            .unwrap();
        let ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
            panic!("chain did not start");
        };
        chain_id
    }
}

#[tokio::test]
async fn terminal_history_replays_the_receipted_event_after_a_lost_acknowledgement() {
    let fixture = Fixture::new();
    let chain_id = fixture.start().await;

    // The receipt commits but its acknowledgement is lost before the normal
    // event key is written. A later sweep must use that receipt instead of
    // allocating and appending another cancellation event.
    fixture
        .fault
        .fail_next(
            KeyKind::Custom("exec_history_terminal".into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    fixture
        .gateway
        .cancel_chain(
            NAMESPACE,
            TENANT,
            &chain_id,
            Some("operator request".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(fixture.fault.consumed(), 1);
    assert!(
        !fixture
            .gateway
            .get_execution_history(NAMESPACE, TENANT, &chain_id)
            .await
            .unwrap()
            .events
            .iter()
            .any(|entry| matches!(&entry.event, ExecutionEventType::ExecutionCancelled { .. }))
    );

    assert_eq!(
        fixture
            .gateway
            .reconcile_chain_terminal_histories()
            .await
            .unwrap(),
        1
    );
    let history = fixture
        .gateway
        .get_execution_history(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap();
    let cancelled: Vec<_> = history
        .events
        .iter()
        .filter(|entry| {
            matches!(
                &entry.event,
                ExecutionEventType::ExecutionCancelled { reason: Some(reason) }
                    if reason == "operator request"
            )
        })
        .collect();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        fixture
            .gateway
            .reconcile_chain_terminal_histories()
            .await
            .unwrap(),
        0
    );
}
