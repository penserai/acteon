use std::{sync::Arc, time::Duration};

use acteon_core::{
    Action, ActionOutcome, ChainStatus,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig, WorkerStepConfig},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state::{
    KeyKind, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::ManualClock;
use serde_json::json;

const NAMESPACE: &str = "ns";
const TENANT: &str = "tenant";
const WORKER_TASK_KIND: &str = "worker_task";

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
        Self {
            clock: clock.clone(),
            fault: Arc::new(FaultStore::new(state.clone())),
            state,
        }
    }

    fn gateway(&self, chains: Vec<ChainConfig>, chain_name: &str) -> Gateway {
        let mut builder = GatewayBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(
                self.clock.clone(),
            )));
        for chain in chains {
            builder = builder.chain(chain);
        }
        builder
            .rules(vec![Rule::new(
                "start",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: chain_name.into(),
                },
            )])
            .build()
            .unwrap()
    }

    async fn start(&self, gateway: &Gateway) -> String {
        let outcome = gateway
            .dispatch(
                Action::new(NAMESPACE, TENANT, "test", "start", json!({})),
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
async fn retry_adopts_worker_task_created_before_parent_wait() {
    let f = Fixture::new();
    let gateway = f.gateway(
        vec![
            ChainConfig::new("parent").with_step(ChainStepConfig::new_worker(
                "work",
                WorkerStepConfig {
                    queue: "jobs".into(),
                    action_type: Some("compile".into()),
                    timeout_seconds: None,
                    max_attempts: Some(1),
                },
                json!({"source":"chain"}),
            )),
        ],
        "parent",
    );
    let parent_id = f.start(&gateway).await;

    // The task and its queue discovery commit first. Interrupt the following
    // parent CAS, which otherwise records `WaitingWorker` and the task ID.
    f.fault
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        gateway
            .advance_chain(NAMESPACE, TENANT, &parent_id)
            .await
            .is_err()
    );
    // The interrupted call did not reach its normal lock-release path.
    // Advance past the production lease before simulating a new process.
    f.clock.advance_to(Duration::from_secs(61)).unwrap();
    assert_eq!(f.fault.consumed(), 1);
    let rows = f
        .state
        .scan_keys_by_kind(KeyKind::Custom(WORKER_TASK_KIND.into()))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        gateway
            .get_chain_status(NAMESPACE, TENANT, &parent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ChainStatus::Running
    );

    gateway
        .advance_chain(NAMESPACE, TENANT, &parent_id)
        .await
        .unwrap();
    let parent = gateway
        .get_chain_status(NAMESPACE, TENANT, &parent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, ChainStatus::WaitingWorker);
    assert_eq!(
        f.state
            .scan_keys_by_kind(KeyKind::Custom(WORKER_TASK_KIND.into()))
            .await
            .unwrap()
            .len(),
        1,
        "retry must adopt the first task rather than enqueue a duplicate"
    );
}

#[tokio::test]
async fn cancellation_discovers_child_created_before_parent_link() {
    let f = Fixture::new();
    let parent =
        ChainConfig::new("parent").with_step(ChainStepConfig::new_sub_chain("spawn", "child"));
    let child = ChainConfig::new("child").with_step(ChainStepConfig::new_timer(
        "wait",
        TimerStepConfig {
            duration_seconds: Some(60),
            until: None,
        },
    ));
    let gateway = f.gateway(vec![parent, child], "parent");
    let parent_id = f.start(&gateway).await;

    // The child primary row and discovery commit before the parent can store
    // its reverse child ID. Fail that parent CAS to reproduce the gap.
    f.fault
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        gateway
            .advance_chain(NAMESPACE, TENANT, &parent_id)
            .await
            .is_err()
    );
    f.clock.advance_to(Duration::from_secs(61)).unwrap();
    let rows = f.state.scan_keys_by_kind(KeyKind::Chain).await.unwrap();
    assert_eq!(rows.len(), 2);
    let child_id = rows
        .iter()
        .map(|(key, _)| key.rsplit(':').next().unwrap())
        .find(|id| *id != parent_id.as_str())
        .unwrap()
        .to_owned();
    assert!(
        gateway
            .get_chain_status(NAMESPACE, TENANT, &parent_id)
            .await
            .unwrap()
            .unwrap()
            .child_chain_ids
            .is_empty()
    );

    gateway
        .cancel_chain(NAMESPACE, TENANT, &parent_id, None, None)
        .await
        .unwrap();
    assert_eq!(
        gateway
            .get_chain_status(NAMESPACE, TENANT, &child_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ChainStatus::Cancelled,
        "cancellation must use the child primary relation when the parent link is absent"
    );
}
