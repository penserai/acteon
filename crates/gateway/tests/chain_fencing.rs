use acteon_core::ProviderResponse;
use acteon_core::{
    Action, ActionOutcome, ChainState, ChainStatus,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
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
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Effect {
    calls: AtomicUsize,
    paused: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait::async_trait]
impl DynProvider for Effect {
    fn name(&self) -> &'static str {
        "effect"
    }
    async fn execute(&self, _: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let paused = self.paused.lock().unwrap().take();
        if let Some(paused) = paused {
            paused.await.unwrap();
        }
        Ok(ProviderResponse::success(json!({"effect": true})))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
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
struct Fixture {
    clock: Arc<ManualClock>,
    state: Arc<MemoryStateStore>,
    fault: Arc<FaultStore>,
    gateway: Gateway,
    effect: Arc<Effect>,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(timer(), false)
    }
    fn configured(step: ChainStepConfig, encrypted: bool) -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let state = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let fault = Arc::new(FaultStore::new(state.clone()));
        let config = ChainConfig::new("flow").with_step(step);
        let effect = Arc::new(Effect {
            calls: AtomicUsize::new(0),
            paused: Mutex::new(None),
        });
        let mut builder = GatewayBuilder::new()
            .executor_config(acteon_executor::ExecutorConfig {
                execution_timeout: Duration::from_secs(120),
                max_retries: 0,
                ..Default::default()
            })
            .provider(effect.clone())
            .state(fault.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
            .clock(clock.clone())
            .chain(config)
            .rules(vec![Rule::new(
                "start",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: "flow".into(),
                },
            )]);
        if encrypted {
            builder = builder.payload_encryptor(Arc::new(acteon_crypto::PayloadEncryptor::new(
                acteon_crypto::parse_master_key(&"47".repeat(32)).unwrap(),
            )));
        }
        let gateway = builder.build().unwrap();
        Self {
            clock,
            state,
            fault,
            gateway,
            effect,
        }
    }
    async fn start(&self) -> String {
        let outcome = self
            .gateway
            .dispatch(
                Action::new("ns", "tenant", "effect", "start", json!({})),
                None,
            )
            .await
            .unwrap();
        let ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
            panic!("expected chain")
        };
        chain_id
    }
    async fn chain(&self, id: &str) -> ChainState {
        self.gateway
            .get_chain_status("ns", "tenant", id)
            .await
            .unwrap()
            .unwrap()
    }
    fn pause(&self) -> tokio::sync::oneshot::Sender<()> {
        self.fault
            .pause_next(
                KeyKind::Chain,
                WriteOperation::CompareAndSwap,
                FaultTiming::Before,
            )
            .unwrap()
    }
}

#[tokio::test]
async fn late_timer_arming_cannot_resurrect_a_cancelled_chain() {
    let f = Fixture::new();
    let id = f.start().await;
    let resume = f.pause();
    let mut advance = Box::pin(f.gateway.advance_chain("ns", "tenant", &id));
    assert!(poll!(&mut advance).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.clock.advance_to(Duration::from_secs(60)).unwrap();
    f.gateway
        .cancel_chain("ns", "tenant", &id, Some("operator".into()), None)
        .await
        .unwrap();
    resume.send(()).unwrap();
    assert!(advance.await.is_err());
    let current = f.chain(&id).await;
    assert_eq!(current.status, ChainStatus::Cancelled);
    assert!(current.wait_state.is_none());
}

#[tokio::test]
async fn stale_search_update_cannot_erase_cancellation() {
    let f = Fixture::new();
    let id = f.start().await;
    let resume = f.pause();
    let mut update = Box::pin(f.gateway.upsert_search_attributes(
        "ns",
        "tenant",
        &id,
        HashMap::from([("old".into(), json!(true))]),
    ));
    assert!(poll!(&mut update).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.clock.advance_to(Duration::from_secs(30)).unwrap();
    f.gateway
        .cancel_chain("ns", "tenant", &id, Some("operator".into()), None)
        .await
        .unwrap();
    resume.send(()).unwrap();
    assert!(update.await.is_err());
    assert_eq!(f.chain(&id).await.status, ChainStatus::Cancelled);
}

#[tokio::test]
async fn stale_cancellation_cannot_overwrite_reset() {
    let f = Fixture::new();
    let id = f.start().await;
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    let resume = f.pause();
    let mut cancel = Box::pin(f.gateway.cancel_chain("ns", "tenant", &id, None, None));
    assert!(poll!(&mut cancel).is_pending());
    f.clock.advance_to(Duration::from_secs(30)).unwrap();
    f.gateway
        .reset_execution("ns", "tenant", &id, "nap", None)
        .await
        .unwrap();
    let expected = serde_json::to_value(f.chain(&id).await).unwrap();
    resume.send(()).unwrap();
    assert!(cancel.await.is_err());
    assert_eq!(serde_json::to_value(f.chain(&id).await).unwrap(), expected);
}

#[tokio::test]
async fn stale_reset_cannot_discard_completed_results() {
    let f = Fixture::new();
    let id = f.start().await;
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    let resume = f.pause();
    let mut reset = Box::pin(f.gateway.reset_execution("ns", "tenant", &id, "nap", None));
    assert!(poll!(&mut reset).is_pending());
    f.clock.advance_to(Duration::from_secs(30)).unwrap();
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    let expected = serde_json::to_value(f.chain(&id).await).unwrap();
    resume.send(()).unwrap();
    assert!(reset.await.is_err());
    assert_eq!(f.chain(&id).await.status, ChainStatus::Completed);
    assert_eq!(serde_json::to_value(f.chain(&id).await).unwrap(), expected);
}

#[tokio::test]
async fn deleted_execution_is_not_recreated_by_an_inflight_update() {
    let f = Fixture::new();
    let id = f.start().await;
    let resume = f.pause();
    let mut update =
        Box::pin(
            f.gateway
                .upsert_search_attributes("ns", "tenant", &id, HashMap::new()),
        );
    assert!(poll!(&mut update).is_pending());
    let key = StateKey::new("ns", "tenant", KeyKind::Chain, &id);
    f.state.delete(&key).await.unwrap();
    resume.send(()).unwrap();
    assert!(update.await.is_err());
    assert!(f.state.get(&key).await.unwrap().is_none());
}

fn parallel() -> ChainStepConfig {
    ChainStepConfig::new_parallel(
        "group",
        acteon_core::chain::ParallelStepGroup {
            steps: vec![
                ChainStepConfig::new("one", "effect", "run", json!({})),
                ChainStepConfig::new("two", "effect", "run", json!({})),
            ],
            join: acteon_core::chain::ParallelJoinPolicy::default(),
            on_failure: acteon_core::chain::ParallelFailurePolicy::default(),
            timeout_seconds: None,
            max_concurrency: None,
        },
    )
}

#[tokio::test]
async fn late_provider_and_parallel_results_preserve_cancellation() {
    for step in [
        ChainStepConfig::new("one", "effect", "run", json!({})),
        parallel(),
    ] {
        let f = Fixture::configured(step, true);
        let id = f.start().await;
        let (resume, paused) = tokio::sync::oneshot::channel();
        *f.effect.paused.lock().unwrap() = Some(paused);
        let mut advance = Box::pin(f.gateway.advance_chain("ns", "tenant", &id));
        assert!(poll!(&mut advance).is_pending());
        assert!(f.effect.calls.load(Ordering::SeqCst) > 0);
        f.clock.advance_to(Duration::from_secs(60)).unwrap();
        f.gateway
            .cancel_chain("ns", "tenant", &id, None, None)
            .await
            .unwrap();
        let expected = serde_json::to_value(f.chain(&id).await).unwrap();
        resume.send(()).unwrap();
        assert!(advance.await.is_err());
        assert_eq!(serde_json::to_value(f.chain(&id).await).unwrap(), expected);
        let raw = f
            .state
            .get(&StateKey::new("ns", "tenant", KeyKind::Chain, &id))
            .await
            .unwrap()
            .unwrap();
        assert!(acteon_crypto::is_encrypted(&raw));
    }
}

#[tokio::test]
async fn successive_parallel_writes_track_versions_without_changing_json() {
    let f = Fixture::configured(parallel(), true);
    let id = f.start().await;
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    let state = f.chain(&id).await;
    assert_eq!(state.status, ChainStatus::Completed);
    assert_eq!(state.parallel_sub_results.len(), 2);
    assert!(state.state_version.unwrap() >= 3);
    let json = serde_json::to_value(state).unwrap();
    assert!(json.get("state_version").is_none());
    let legacy: ChainState = serde_json::from_value(json).unwrap();
    assert!(legacy.state_version.is_none());
}

#[tokio::test]
async fn retention_cannot_delete_an_execution_reset_after_its_snapshot() {
    let f = Fixture::new();
    let id = f.start().await;
    f.gateway
        .cancel_chain("ns", "tenant", &id, None, None)
        .await
        .unwrap();
    f.clock.advance_to(Duration::from_secs(60)).unwrap();
    let policy: acteon_core::RetentionPolicy = serde_json::from_value(json!({
        "id":"retention", "namespace":"ns", "tenant":"tenant", "state_ttl_seconds":10,
        "created_at": f.clock.now(), "updated_at":f.clock.now(),
    }))
    .unwrap();
    f.state
        .set(
            &StateKey::new("ns", "tenant", KeyKind::Retention, "retention"),
            &serde_json::to_string(&policy).unwrap(),
            None,
        )
        .await
        .unwrap();
    let (worker, _) = BackgroundProcessorBuilder::new()
        .config(acteon_gateway::BackgroundConfig {
            enable_retention_reaper: true,
            ..Default::default()
        })
        .clock(f.clock.clone())
        .state(f.fault.clone())
        .group_manager(f.gateway.group_manager())
        .metrics(f.gateway.metrics_arc())
        .build()
        .unwrap();
    let mut worker = worker.with_retention_policies(HashMap::from([("ns:tenant".into(), policy)]));
    let resume = f
        .fault
        .pause_next(
            KeyKind::Chain,
            WriteOperation::CompareAndDelete,
            FaultTiming::Before,
        )
        .unwrap();
    let mut reap = Box::pin(worker.tick(BackgroundJob::Retention));
    assert!(poll!(&mut reap).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.gateway
        .reset_execution("ns", "tenant", &id, "nap", None)
        .await
        .unwrap();
    resume.send(()).unwrap();
    reap.await.unwrap();
    assert_eq!(f.chain(&id).await.status, ChainStatus::Running);
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    f.clock.advance_to(Duration::from_secs(65)).unwrap();
    f.gateway.advance_chain("ns", "tenant", &id).await.unwrap();
    worker.tick(BackgroundJob::Retention).await.unwrap();
    assert!(
        f.gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn mismatched_chain_scope_is_rejected_without_mutating_the_record() {
    let f = Fixture::new();
    let id = f.start().await;
    let raw = f
        .state
        .get(&StateKey::new("ns", "tenant", KeyKind::Chain, &id))
        .await
        .unwrap()
        .unwrap();
    let wrong = StateKey::new("ns", "other", KeyKind::Chain, &id);
    f.state.set(&wrong, &raw, None).await.unwrap();
    assert!(
        f.gateway
            .upsert_search_attributes("ns", "other", &id, HashMap::new())
            .await
            .is_err()
    );
    assert_eq!(f.state.get(&wrong).await.unwrap().unwrap(), raw);
}
