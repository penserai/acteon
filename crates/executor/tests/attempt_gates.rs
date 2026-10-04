use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, ProviderResponse, ResourceKind, ResourceRef};
use acteon_executor::{
    ActionExecutor, AttemptGateError, AttemptSettlement, ExecutorConfig, ProviderAttempt,
    ProviderAttemptGate, ProviderAttemptOutcome, RegisteredProviderAttempt, RetryStrategy,
};
use acteon_governance::{
    AttemptRequest, AttemptStatus, AuthorityChange, AuthorityCoordinator, CoordinatorLimits,
    RootBudgetLimits, RootReservation, StartRegistration,
};
use acteon_provider::{DispatchContext, DynProvider, ProviderError};
use acteon_state_memory::MemoryStateStore;
use async_trait::async_trait;
use tokio::sync::{Notify, Semaphore};

// This contract adapter exercises real coordinator state at provider boundaries.
// It is not a production permit evaluator: the fixture supplies trusted roots
// and one explicitly chosen complete provider/endpoint tuple.
struct Gate {
    coordinator: AuthorityCoordinator,
    starts: AtomicUsize,
    finished: Arc<Notify>,
    fail_finish: bool,
}
struct Registered {
    finished: Arc<Notify>,
    coordinator: AuthorityCoordinator,
    id: String,
    token: String,
    fail_finish: bool,
}
fn resources() -> Vec<ResourceRef> {
    vec![
        ResourceRef::new(ResourceKind::Provider, "ns", "t", "selected").unwrap(),
        ResourceRef::new(ResourceKind::Endpoint, "ns", "t", "selected-http").unwrap(),
    ]
}
#[async_trait]
impl ProviderAttemptGate for Gate {
    async fn start(
        &self,
        attempt: ProviderAttempt<'_>,
    ) -> Result<Box<dyn RegisteredProviderAttempt>, AttemptGateError> {
        assert_eq!(attempt.provider_name, "selected");
        assert_eq!(attempt.action.provider.as_str(), "original");
        assert_eq!(
            attempt.ordinal as usize,
            self.starts.fetch_add(1, Ordering::SeqCst)
        );
        let stamp = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| AttemptGateError::Unavailable)?
            .stamp();
        let id = uuid::Uuid::new_v4().to_string();
        let registration = self
            .coordinator
            .register_attempt(AttemptRequest {
                id: &id,
                subject: "actor",
                resources: &resources(),
                request_digest: "test-input",
                expected_authority: &stamp,
                reservation: Some(RootReservation {
                    root_id: "root".into(),
                    units: 1,
                }),
                now_ms: attempt.now_ms,
            })
            .await
            .map_err(|_| AttemptGateError::Denied)?;
        let StartRegistration::New(record) = registration else {
            return Err(AttemptGateError::Conflict);
        };
        Ok(Box::new(Registered {
            finished: self.finished.clone(),
            coordinator: self.coordinator.clone(),
            id,
            token: record.token,
            fail_finish: self.fail_finish,
        }))
    }
}
#[async_trait]
impl RegisteredProviderAttempt for Registered {
    async fn finish(
        self: Box<Self>,
        outcome: ProviderAttemptOutcome<'_>,
    ) -> Result<AttemptSettlement, AttemptGateError> {
        if self.fail_finish {
            return Err(AttemptGateError::Unavailable);
        }
        // This test provider guarantees rate-limit rejection did not perform an
        // effect. Production adapters need a qualified contract for that claim.
        let known = matches!(
            outcome,
            ProviderAttemptOutcome::Succeeded(_)
                | ProviderAttemptOutcome::Failed(ProviderError::RateLimited)
        );
        self.coordinator
            .settle(
                &self.id,
                &self.token,
                if known {
                    AttemptStatus::Settled
                } else {
                    AttemptStatus::Uncertain
                },
            )
            .await
            .map_err(|_| AttemptGateError::Unavailable)?;
        self.finished.notify_one();
        Ok(if known {
            AttemptSettlement::Settled
        } else {
            AttemptSettlement::Uncertain
        })
    }
}
enum Mode {
    Success,
    RateLimitFirst,
    Connection,
    Block,
}
struct Provider {
    mode: Mode,
    calls: AtomicUsize,
    entered: Notify,
    release: Semaphore,
}
impl Provider {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
            release: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl DynProvider for Provider {
    fn name(&self) -> &'static str {
        "selected"
    }
    async fn execute(&self, _: &Action) -> Result<ProviderResponse, ProviderError> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        match self.mode {
            Mode::Connection => {
                return Err(ProviderError::Connection(
                    "untrusted sensitive detail".into(),
                ));
            }
            Mode::RateLimitFirst if ordinal == 0 => return Err(ProviderError::RateLimited),
            Mode::Block => {
                self.release.acquire().await.unwrap().forget();
            }
            _ => {}
        }
        Ok(ProviderResponse::success(serde_json::json!({"ok":true})))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
fn action() -> Action {
    Action::new("ns", "t", "original", "test", serde_json::Value::Null)
}
fn config() -> ExecutorConfig {
    ExecutorConfig {
        max_concurrent: 1,
        max_retries: 2,
        retry_strategy: RetryStrategy::Constant {
            delay: Duration::ZERO,
        },
        ..ExecutorConfig::default()
    }
}
async fn gate() -> Gate {
    let coordinator = AuthorityCoordinator::initialize(
        Arc::new(MemoryStateStore::new()),
        "ns",
        "t",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    coordinator
        .create_root_budget(
            "root",
            "actor",
            RootBudgetLimits {
                max_units: 10,
                max_concurrent: 2,
                deadline_ms: i64::MAX,
            },
            &coordinator.snapshot().await.unwrap().stamp(),
            0,
        )
        .await
        .unwrap();
    Gate {
        coordinator,
        starts: AtomicUsize::new(0),
        finished: Arc::new(Notify::new()),
        fail_finish: false,
    }
}
fn failed(outcome: ActionOutcome, code: &str, attempts: u32) {
    let ActionOutcome::Failed(error) = outcome else {
        panic!("expected failure")
    };
    assert_eq!(error.code, code);
    assert_eq!(error.attempts, attempts);
    assert!(!error.retryable);
    assert!(!error.message.contains("sensitive"));
}

#[tokio::test]
async fn exhausted_governed_work_does_not_lose_authority_in_the_legacy_dlq() {
    let gate = gate().await;
    let provider = Provider::new(Mode::RateLimitFirst);
    let dlq = Arc::new(acteon_executor::DeadLetterQueue::new());
    let mut settings = config();
    settings.max_retries = 0;
    let executor = ActionExecutor::with_dlq(settings, dlq.clone()).require_attempt_gate();
    failed(
        executor
            .execute_with_gate(&action(), &provider, None, &gate)
            .await,
        "RATE_LIMITED",
        1,
    );
    assert!(dlq.drain().is_empty());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gate.coordinator.snapshot().await.unwrap().roots["root"].active_attempts,
        0
    );
}

#[tokio::test]
async fn required_gate_rejects_all_legacy_entrypoints_without_provider_calls() {
    let executor = ActionExecutor::new(config()).require_attempt_gate();
    let provider = Provider::new(Mode::Success);
    let action = action();
    failed(
        executor.execute(&action, &provider).await,
        "ATTEMPT_GATE_REQUIRED",
        0,
    );
    failed(
        executor
            .execute_with_context(&action, &provider, &DispatchContext::default())
            .await,
        "ATTEMPT_GATE_REQUIRED",
        0,
    );
    for result in acteon_executor::batch::execute_batch(&executor, &[action], &provider).await {
        failed(result, "ATTEMPT_GATE_REQUIRED", 0);
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_provider_and_each_retry_get_new_registered_and_settled_attempts() {
    let gate = gate().await;
    let provider = Provider::new(Mode::RateLimitFirst);
    let outcome = ActionExecutor::new(config())
        .require_attempt_gate()
        .execute_with_gate(
            &action(),
            &provider,
            Some(&DispatchContext::default()),
            &gate,
        )
        .await;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(state.starts.len(), 2);
    assert_eq!(state.roots["root"].spent_units, 2);
    assert_eq!(state.roots["root"].active_attempts, 0);
    assert!(
        state
            .starts
            .values()
            .all(|s| s.resources.contains(&resources()[1]))
    );
}

#[tokio::test]
async fn ambiguous_connection_error_retains_capacity_and_prevents_automatic_retry() {
    let gate = gate().await;
    let provider = Provider::new(Mode::Connection);
    failed(
        ActionExecutor::new(config())
            .execute_with_gate(&action(), &provider, None, &gate)
            .await,
        "ATTEMPT_UNCERTAIN",
        1,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].spent_units, 1);
    assert_eq!(state.roots["root"].active_attempts, 1);
    assert_eq!(
        state.starts.values().next().unwrap().status,
        AttemptStatus::Uncertain
    );
}

#[tokio::test]
async fn settlement_failure_preserves_registration_and_does_not_repeat_successful_effect() {
    let mut gate = gate().await;
    gate.fail_finish = true;
    let provider = Provider::new(Mode::Success);
    failed(
        ActionExecutor::new(config())
            .execute_with_gate(&action(), &provider, None, &gate)
            .await,
        "ATTEMPT_SETTLEMENT_REQUIRED",
        1,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].active_attempts, 1);
    assert_eq!(
        state.starts.values().next().unwrap().status,
        AttemptStatus::InFlight
    );
}

#[tokio::test]
async fn cancellation_after_registration_leaves_an_unresolved_attempt() {
    let gate = Arc::new(gate().await);
    let provider = Arc::new(Provider::new(Mode::Block));
    let task = {
        let gate = gate.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            ActionExecutor::new(config())
                .execute_with_gate(&action(), provider.as_ref(), None, gate.as_ref())
                .await
        })
    };
    provider.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].active_attempts, 1);
    assert_eq!(
        state.starts.values().next().unwrap().status,
        AttemptStatus::InFlight
    );
}

#[tokio::test]
async fn closure_during_semaphore_wait_denies_the_waiting_effect() {
    let gate = Arc::new(gate().await);
    let provider = Arc::new(Provider::new(Mode::Block));
    let executor = Arc::new(ActionExecutor::new(config()).require_attempt_gate());
    let first = {
        let gate = gate.clone();
        let provider = provider.clone();
        let executor = executor.clone();
        tokio::spawn(async move {
            executor
                .execute_with_gate(&action(), provider.as_ref(), None, gate.as_ref())
                .await
        })
    };
    provider.entered.notified().await;
    // A separate root/call adapter has its own ordinal sequence.
    let waiting_gate = Arc::new(Gate {
        coordinator: gate.coordinator.clone(),
        starts: AtomicUsize::new(0),
        finished: Arc::new(Notify::new()),
        fail_finish: false,
    });
    let second = {
        let gate = waiting_gate.clone();
        let provider = provider.clone();
        let executor = executor.clone();
        tokio::spawn(async move {
            executor
                .execute_with_gate(&action(), provider.as_ref(), None, gate.as_ref())
                .await
        })
    };
    tokio::task::yield_now().await;
    assert_eq!(waiting_gate.starts.load(Ordering::SeqCst), 0);
    gate.coordinator
        .change(
            "closure",
            AuthorityChange::CloseResource {
                resource: resources()[1].clone(),
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
    provider.release.add_permits(1);
    assert!(matches!(first.await.unwrap(), ActionOutcome::Executed(_)));
    failed(second.await.unwrap(), "ATTEMPT_DENIED", 0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gate.coordinator.snapshot().await.unwrap().roots["root"].spent_units,
        1
    );
}

#[tokio::test]
async fn revocation_during_backoff_prevents_the_next_retry() {
    let clock = Arc::new(acteon_time::ManualClock::new(chrono::Utc::now()));
    let gate = Arc::new(gate().await);
    let provider = Arc::new(Provider::new(Mode::RateLimitFirst));
    let mut settings = config();
    settings.retry_strategy = RetryStrategy::Constant {
        delay: Duration::from_secs(10),
    };
    let task = {
        let gate = gate.clone();
        let provider = provider.clone();
        let clock = clock.clone();
        tokio::spawn(async move {
            ActionExecutor::new(settings)
                .clock(clock)
                .execute_with_gate(&action(), provider.as_ref(), None, gate.as_ref())
                .await
        })
    };
    gate.finished.notified().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while clock.next_deadline() != Some(Duration::from_secs(10)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    gate.coordinator
        .change(
            "revoke",
            AuthorityChange::RevokeSubject {
                subject: "actor".into(),
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
    clock.advance_to(Duration::from_secs(10)).unwrap();
    failed(task.await.unwrap(), "ATTEMPT_DENIED", 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].spent_units, 1);
    assert_eq!(state.roots["root"].active_attempts, 0);
}

#[tokio::test]
async fn timeout_is_uncertain_and_cannot_release_capacity_for_an_automatic_retry() {
    let clock = Arc::new(acteon_time::ManualClock::new(chrono::Utc::now()));
    let gate = Arc::new(gate().await);
    let provider = Arc::new(Provider::new(Mode::Block));
    let task = {
        let gate = gate.clone();
        let provider = provider.clone();
        let clock = clock.clone();
        tokio::spawn(async move {
            ActionExecutor::new(config())
                .clock(clock)
                .execute_with_gate(&action(), provider.as_ref(), None, gate.as_ref())
                .await
        })
    };
    provider.entered.notified().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while clock.next_deadline() != Some(Duration::from_secs(30)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    clock.advance_to(Duration::from_secs(30)).unwrap();
    failed(task.await.unwrap(), "ATTEMPT_UNCERTAIN", 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let state = gate.coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].active_attempts, 1);
    assert_eq!(
        state.starts.values().next().unwrap().status,
        AttemptStatus::Uncertain
    );
}
