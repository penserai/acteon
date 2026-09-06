use std::sync::Arc;
use std::time::Duration;

use acteon_core::{
    Action, ChainStatus,
    chain::{ChainConfig, ChainStepConfig, SignalStepConfig, TimerStepConfig},
};
use acteon_gateway::{BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use futures::poll;
use serde_json::json;

struct Fixture {
    clock: Arc<ManualClock>,
    state: Arc<MemoryStateStore>,
    fault: Arc<FaultStore>,
}

impl Fixture {
    fn new() -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let state = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let fault = Arc::new(FaultStore::new(state.clone()));
        Self {
            clock,
            state,
            fault,
        }
    }

    fn gateway(&self, step: ChainStepConfig) -> Gateway {
        GatewayBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(
                self.clock.clone(),
            )))
            .chain(ChainConfig::new("flow").with_step(step))
            .rules(vec![Rule::new(
                "start",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: "flow".into(),
                },
            )])
            .build()
            .unwrap()
    }

    async fn cleanup(&self, gateway: Gateway) {
        let gateway = Arc::new(tokio::sync::RwLock::new(gateway));
        let group_manager = gateway.read().await.group_manager();
        let metrics = gateway.read().await.metrics_arc();
        let (mut worker, _) = BackgroundProcessorBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .group_manager(group_manager)
            .metrics(metrics)
            .gateway(gateway.clone())
            .build()
            .unwrap();
        worker.tick(BackgroundJob::Cleanup).await.unwrap();
    }

    async fn start(&self, gateway: &Gateway) -> String {
        let outcome = gateway
            .dispatch(
                Action::new("ns", "tenant", "timer", "start", json!({})),
                None,
            )
            .await
            .unwrap();
        let acteon_core::ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
            panic!("chain did not start")
        };
        chain_id
    }
}

fn timer() -> ChainStepConfig {
    ChainStepConfig::new_timer(
        "nap",
        TimerStepConfig {
            duration_seconds: Some(5),
            until: None,
        },
    )
}

fn signal() -> ChainStepConfig {
    ChainStepConfig::new_wait_for_signal(
        "wait",
        SignalStepConfig {
            signal_name: "continue".into(),
            timeout_seconds: None,
            on_timeout: None,
        },
    )
}

fn pending(id: &str) -> StateKey {
    StateKey::new("ns", "tenant", KeyKind::PendingChains, id)
}

#[tokio::test]
async fn cleanup_repairs_interrupted_chain_start_discovery() {
    let f = Fixture::new();
    let gateway = f.gateway(timer());
    f.fault
        .fail_next(
            KeyKind::PendingChains,
            WriteOperation::Set,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        gateway
            .dispatch(
                Action::new("ns", "tenant", "timer", "start", json!({})),
                None
            )
            .await
            .is_err()
    );
    assert_eq!(f.fault.consumed(), 1);
    let rows = f.state.scan_keys_by_kind(KeyKind::Chain).await.unwrap();
    assert_eq!(rows.len(), 1);
    let id = rows[0].0.rsplit(':').next().unwrap().to_owned();
    assert!(f.state.get(&pending(&id)).await.unwrap().is_none());

    drop(gateway);
    f.cleanup(f.gateway(timer())).await;

    assert!(f.state.get(&pending(&id)).await.unwrap().is_some());
    assert!(
        f.state
            .get_ready_chains(f.clock.now().timestamp_millis())
            .await
            .unwrap()
            .iter()
            .any(|key| key.ends_with(&id))
    );
}

#[tokio::test]
async fn cleanup_rebuilds_a_lost_buffered_signal_wake() {
    let f = Fixture::new();
    let gateway = f.gateway(signal());
    let id = f.start(&gateway).await;
    gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    assert_eq!(
        gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ChainStatus::WaitingSignal
    );
    gateway
        .signal_chain("ns", "tenant", &id, "continue", json!({"go":true}))
        .await
        .unwrap();
    f.state.delete(&pending(&id)).await.unwrap();
    f.state
        .remove_chain_ready_index(&pending(&id))
        .await
        .unwrap();

    drop(gateway);
    f.cleanup(f.gateway(signal())).await;
    assert!(f.state.get(&pending(&id)).await.unwrap().is_some());
    assert!(
        f.state
            .get_ready_chains(f.clock.now().timestamp_millis())
            .await
            .unwrap()
            .iter()
            .any(|key| key.ends_with(&id))
    );
}

#[tokio::test]
async fn delayed_terminal_cleanup_preserves_reset_discovery() {
    let f = Fixture::new();
    let gateway = f.gateway(timer());
    let id = f.start(&gateway).await;
    gateway.advance_chain("ns", "tenant", &id).await.unwrap();

    let resume = f
        .fault
        .pause_next(
            KeyKind::PendingChains,
            WriteOperation::Delete,
            FaultTiming::Before,
        )
        .unwrap();
    let mut cancel = Box::pin(gateway.cancel_chain("ns", "tenant", &id, None, None));
    assert!(poll!(&mut cancel).is_pending());
    f.clock.advance_to(Duration::from_secs(31)).unwrap();
    gateway
        .reset_execution("ns", "tenant", &id, "nap", None)
        .await
        .unwrap();
    resume.send(()).unwrap();
    cancel.await.unwrap();

    assert_eq!(
        gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ChainStatus::Running
    );
    assert!(f.state.get(&pending(&id)).await.unwrap().is_some());
    assert!(
        f.state
            .get_ready_chains(f.clock.now().timestamp_millis())
            .await
            .unwrap()
            .iter()
            .any(|key| key.ends_with(&id))
    );
}
