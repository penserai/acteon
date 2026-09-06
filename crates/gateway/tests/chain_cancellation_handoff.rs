//! Fault-injection coverage for the durable cancellation notification outbox.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{
    Action, ActionOutcome, ChainStatus, ProviderResponse,
    chain::{ChainConfig, ChainNotificationTarget, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::{
    expr::{BinaryOp, Expr},
    rule::{Rule, RuleAction},
};
use acteon_state::{
    KeyKind,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::ManualClock;
use serde_json::json;

const NAMESPACE: &str = "ns";
const TENANT: &str = "tenant";

struct NotificationProvider {
    unavailable: AtomicBool,
    calls: AtomicUsize,
    delivery_ids: Mutex<Vec<String>>,
    acknowledgement_fault: Mutex<Option<Arc<FaultStore>>>,
}

impl NotificationProvider {
    fn new() -> Self {
        Self {
            unavailable: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            delivery_ids: Mutex::new(Vec::new()),
            acknowledgement_fault: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl DynProvider for NotificationProvider {
    fn name(&self) -> &'static str {
        "notify"
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.delivery_ids
            .lock()
            .unwrap()
            .push(action.id.to_string());
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(ProviderError::Connection(
                "injected notification outage".into(),
            ));
        }
        if let Some(fault) = self.acknowledgement_fault.lock().unwrap().take() {
            fault
                .fail_next(
                    KeyKind::Chain,
                    WriteOperation::CompareAndSwap,
                    FaultTiming::Before,
                )
                .unwrap();
        }
        Ok(ProviderResponse::success(json!({"notified": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct Fixture {
    clock: Arc<ManualClock>,
    fault: Arc<FaultStore>,
    gateway: Gateway,
    notifications: Arc<NotificationProvider>,
}

impl Fixture {
    fn new() -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let state = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let fault = Arc::new(FaultStore::new(state));
        let notifications = Arc::new(NotificationProvider::new());
        let chain = ChainConfig::new("flow")
            .with_step(ChainStepConfig::new_timer(
                "wait",
                TimerStepConfig {
                    duration_seconds: Some(60),
                    until: None,
                },
            ))
            .with_on_cancel(ChainNotificationTarget {
                provider: "notify".into(),
                action_type: "chain_cancelled".into(),
            });
        let gateway = GatewayBuilder::new()
            .clock(clock.clone())
            .state(fault.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
            .provider(notifications.clone())
            .chain(chain)
            .rules(vec![Rule::new(
                "start",
                Expr::Binary(
                    BinaryOp::Eq,
                    Box::new(Expr::Field(
                        Box::new(Expr::Ident("action".into())),
                        "action_type".into(),
                    )),
                    Box::new(Expr::String("start".into())),
                ),
                RuleAction::Chain {
                    chain: "flow".into(),
                },
            )])
            .executor_config(acteon_executor::ExecutorConfig {
                max_retries: 0,
                ..Default::default()
            })
            .build()
            .unwrap();
        Self {
            clock,
            fault,
            gateway,
            notifications,
        }
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
            panic!("expected chain start");
        };
        chain_id
    }
}

#[tokio::test]
async fn cancellation_notification_retries_after_provider_outage() {
    let fixture = Fixture::new();
    fixture
        .notifications
        .unavailable
        .store(true, Ordering::SeqCst);
    let chain_id = fixture.start().await;

    let cancelled = fixture
        .gateway
        .cancel_chain(
            NAMESPACE,
            TENANT,
            &chain_id,
            Some("operator request".into()),
            Some("renzo".into()),
        )
        .await
        .unwrap();
    let delivery_id = cancelled
        .cancellation_handoff
        .as_ref()
        .expect("terminal cancellation stores an outbox record")
        .delivery_id
        .clone();
    assert_eq!(fixture.notifications.calls.load(Ordering::SeqCst), 1);

    fixture
        .notifications
        .unavailable
        .store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .gateway
            .reconcile_chain_cancellation_handoffs()
            .await
            .unwrap(),
        1
    );
    assert_eq!(fixture.notifications.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .notifications
            .delivery_ids
            .lock()
            .unwrap()
            .as_slice(),
        [delivery_id.as_str(), delivery_id.as_str()]
    );

    let recovered = fixture
        .gateway
        .get_chain_status(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.status, ChainStatus::Cancelled);
    let handoff = recovered.cancellation_handoff.unwrap();
    assert_eq!(handoff.delivery_id, delivery_id);
    assert!(handoff.completed_at.is_some());
    assert!(handoff.lease_token.is_none());
}

#[tokio::test]
async fn lost_acknowledgement_retries_with_the_same_delivery_id() {
    let fixture = Fixture::new();
    let chain_id = fixture.start().await;
    *fixture.notifications.acknowledgement_fault.lock().unwrap() = Some(fixture.fault.clone());

    let cancelled = fixture
        .gateway
        .cancel_chain(NAMESPACE, TENANT, &chain_id, None, None)
        .await
        .unwrap();
    let delivery_id = cancelled
        .cancellation_handoff
        .as_ref()
        .expect("terminal cancellation stores an outbox record")
        .delivery_id
        .clone();
    assert_eq!(fixture.fault.consumed(), 1);
    assert_eq!(fixture.notifications.calls.load(Ordering::SeqCst), 1);

    // The provider completed but the acknowledgement write was interrupted.
    // A later process reclaims the expired lease and reuses the stable ID.
    fixture.clock.advance_to(Duration::from_secs(61)).unwrap();
    assert_eq!(
        fixture
            .gateway
            .reconcile_chain_cancellation_handoffs()
            .await
            .unwrap(),
        1
    );
    assert_eq!(fixture.notifications.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture
            .notifications
            .delivery_ids
            .lock()
            .unwrap()
            .as_slice(),
        [delivery_id.as_str(), delivery_id.as_str()]
    );
    assert!(
        fixture
            .gateway
            .get_chain_status(NAMESPACE, TENANT, &chain_id)
            .await
            .unwrap()
            .unwrap()
            .cancellation_handoff
            .unwrap()
            .completed_at
            .is_some()
    );
}
