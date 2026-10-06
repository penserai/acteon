use acteon_core::{
    Action, ActionOutcome, ExecutionContextReference, PrincipalIdentity, PrincipalKind,
    ProviderResponse, ResourceKind, ResourceRef,
};
use acteon_crypto::{PayloadEncryptor, parse_master_key};
use acteon_executor::governed::history::{HistoricalProviderStore, OperationIntegrity};
use acteon_executor::governed::{
    BoundProvider, GovernedProviderError, GovernedProviderExecutor, GovernedProviderStatus,
    OPERATION_KIND, ProviderFailureContract, RESULT_KIND, governed_provider_input_digest,
};
use acteon_executor::{ExecutorConfig, RetryStrategy};
use acteon_executor::{
    GovernedProviderMediator, ProviderExecutionAuthority, ProviderExecutionMediator,
    ProviderInvocation, ProviderInvocationOrigin,
};
use acteon_governance::context::{
    ContextBinding, ContextSigningKey, ExecutionContextHandle, RootContextAdmission,
    TrustedContextStore,
};
use acteon_governance::permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND, CoordinatorLimits, RootBudgetLimits,
};
use acteon_provider::{DynProvider, ProviderError};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

#[derive(Clone, Copy)]
enum Mode {
    Success,
    Connection,
    Block,
    RejectFirst,
}
struct Counting {
    mode: Mode,
    calls: AtomicUsize,
    ids: Mutex<Vec<String>>,
    entered: Notify,
    release: Semaphore,
}
impl Counting {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            calls: AtomicUsize::new(0),
            ids: Mutex::new(Vec::new()),
            entered: Notify::new(),
            release: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl DynProvider for Counting {
    fn name(&self) -> &'static str {
        "selected"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let count = self.calls.fetch_add(1, Ordering::SeqCst);
        self.ids.lock().unwrap().push(action.id.to_string());
        self.entered.notify_one();
        match self.mode {
            Mode::Connection => {
                return Err(ProviderError::Connection("private upstream details".into()));
            }
            Mode::RejectFirst if count == 0 => return Err(ProviderError::RateLimited),
            Mode::Block => self.release.acquire().await.unwrap().forget(),
            _ => {}
        }
        Ok(ProviderResponse::success(
            serde_json::json!({"secret":"receipt-secret","input":action.payload}),
        ))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
struct KnownRateLimit;
impl ProviderFailureContract for KnownRateLimit {
    fn revision(&self) -> &'static str {
        "reviewed-rate-limit-v1"
    }
    fn known_rejected(&self, error: &ProviderError) -> bool {
        matches!(error, ProviderError::RateLimited)
    }
}
fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("agent", PrincipalKind::Agent).unwrap()
}
fn references() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "permit".into(),
        accepted_revision: 1,
    }]
}
fn endpoint() -> ResourceRef {
    ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "endpoint-v1").unwrap()
}
fn config() -> ExecutorConfig {
    ExecutorConfig {
        max_retries: 2,
        max_concurrent: 1,
        execution_timeout: Duration::from_secs(1),
        retry_strategy: RetryStrategy::Constant {
            delay: Duration::ZERO,
        },
    }
}
fn bound(provider: Arc<dyn DynProvider>, known: bool) -> BoundProvider {
    let selected = provider.clone();
    let binding =
        BoundProvider::new_trusted(provider, &endpoint(), "work", "definition-v1", vec![]).unwrap();
    let binding = if known {
        binding
            .with_failure_contract(Arc::new(KnownRateLimit))
            .unwrap()
    } else {
        binding
    };
    let catalog =
        acteon_executor::catalog::QualifiedProviderCatalog::new_trusted(vec![binding]).unwrap();
    let probe = Action::new("city", "tenant", "original", "work", serde_json::json!({}));
    catalog.resolve(&probe, &selected).unwrap().clone()
}
struct Fixture {
    state: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
    clock: Arc<ManualClock>,
    reference: ExecutionContextReference,
    action: Action,
    settings: ExecutorConfig,
    known: bool,
}
impl Fixture {
    fn history(&self, encryptor: Option<Arc<PayloadEncryptor>>) -> HistoricalProviderStore {
        HistoricalProviderStore::new(
            self.state.clone(),
            self.coordinator.clone(),
            self.contexts.clone(),
            encryptor,
        )
    }
    fn driver(
        &self,
        provider: Arc<dyn DynProvider>,
        encryptor: Option<Arc<PayloadEncryptor>>,
    ) -> GovernedProviderExecutor {
        GovernedProviderExecutor::new(
            self.state.clone(),
            self.coordinator.clone(),
            self.contexts.clone(),
            bound(provider, self.known),
            self.settings.clone(),
            self.clock.clone(),
            encryptor,
        )
        .unwrap()
    }
}
async fn fixture(
    state: Arc<dyn StateStore>,
    provider: Arc<dyn DynProvider>,
    known: bool,
    settings: ExecutorConfig,
) -> Fixture {
    fixture_with_scope(state, provider, known, settings, false).await
}
async fn management_fixture(
    state: Arc<dyn StateStore>,
    provider: Arc<dyn DynProvider>,
    known: bool,
    settings: ExecutorConfig,
) -> Fixture {
    fixture_with_scope(state, provider, known, settings, true).await
}
async fn fixture_with_scope(
    state: Arc<dyn StateStore>,
    provider: Arc<dyn DynProvider>,
    known: bool,
    settings: ExecutorConfig,
    reserve_execution: bool,
) -> Fixture {
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp_millis(100).unwrap(),
    ));
    let coordinator = AuthorityCoordinator::initialize(
        state.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    if reserve_execution {
        coordinator
            .reserve_scope(acteon_governance::ScopePurpose::Execution)
            .await
            .unwrap();
    }
    let effects = vec![bound(provider, known).effect().clone()];
    let limits = RootBudgetLimits {
        max_units: 4,
        max_concurrent: 2,
        deadline_ms: 10_000,
    };
    let ceiling = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("issuer", PrincipalKind::Human).unwrap(),
        subjects: vec![actor()],
        effects: effects.clone(),
        valid_from_ms: 0,
        limits: limits.clone(),
    };
    coordinator
        .publish_permit(
            "issue",
            ExecutionPermit {
                id: "permit".into(),
                revision: 1,
                subject: actor(),
                effects: effects.clone(),
                valid_from_ms: 0,
                limits: limits.clone(),
            },
            0,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "reviewed",
            100,
        )
        .await
        .unwrap();
    let contexts = Arc::new(
        TrustedContextStore::new(
            state.clone(),
            coordinator.clone(),
            "domain".into(),
            "k1".into(),
            vec![ContextSigningKey::new("k1".into(), vec![1; 32]).unwrap()],
        )
        .unwrap(),
    );
    let action = Action::new(
        "city",
        "tenant",
        "original",
        "work",
        serde_json::json!({"value":1.000_000_000_000_000_2,"secret":"request-secret"}),
    );
    let context = contexts
        .capture_permitted_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: governed_provider_input_digest(&action).unwrap(),
                },
                credential_id: "key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: "placeholder".into(),
                accepted_effects: effects,
                deadline_ms: 10_000,
                evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
            },
            &references(),
            limits,
            clock.as_ref(),
        )
        .await
        .unwrap();
    Fixture {
        state,
        coordinator,
        contexts,
        clock,
        reference: context.reference().unwrap(),
        action,
        settings,
        known,
    }
}
fn attempt(root: uuid::Uuid, ordinal: u32) -> String {
    uuid::Uuid::new_v5(&root, &ordinal.to_be_bytes()).to_string()
}
fn key(_f: &Fixture, kind: &str, id: String) -> StateKey {
    StateKey::new("city", "tenant", KeyKind::Custom(kind.into()), id)
}
fn completed(status: &GovernedProviderStatus) {
    assert!(matches!(
        status,
        GovernedProviderStatus::Completed {
            outcome: ActionOutcome::Executed(_)
        }
    ));
}
#[tokio::test]
async fn replacement_replays_original_result_and_action_without_a_second_send() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let result = f
        .driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    completed(&result.status);
    let mut delivery = f.action.clone();
    delivery.id = acteon_core::ActionId::new(uuid::Uuid::new_v4().to_string());
    delivery.created_at = chrono::Utc::now();
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &delivery, &actor())
            .await
            .unwrap()
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*provider.ids.lock().unwrap(), vec![f.action.id.to_string()]);
    let state = f.coordinator.snapshot().await.unwrap();
    let start = &state.starts[&attempt(f.reference.execution_id(), 0)];
    assert!(start.evidence.is_some());
    assert_eq!(
        state.roots[&f.reference.execution_id().to_string()].active_attempts,
        0
    );
}
#[tokio::test]
async fn concurrent_workers_compete_for_one_stable_attempt() {
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(
        fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await,
    );
    let first = {
        let f = f.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            f.driver(provider, None)
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
        })
    };
    provider.entered.notified().await;
    let second = f
        .driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    assert!(matches!(
        second.status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    provider.release.add_permits(1);
    completed(&first.await.unwrap().unwrap().status);
}
#[tokio::test]
async fn ambiguous_error_and_cancelled_future_retain_capacity_without_replay() {
    for mode in [Mode::Connection, Mode::Block] {
        let provider = Arc::new(Counting::new(mode));
        let f = Arc::new(
            fixture(
                Arc::new(MemoryStateStore::new()),
                provider.clone(),
                false,
                config(),
            )
            .await,
        );
        if matches!(mode, Mode::Block) {
            let task = {
                let f = f.clone();
                let provider = provider.clone();
                tokio::spawn(async move {
                    f.driver(provider, None)
                        .execute(&f.reference, &references(), &f.action, &actor())
                        .await
                })
            };
            provider.entered.notified().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert!(matches!(
                f.driver(provider.clone(), None)
                    .execute(&f.reference, &references(), &f.action, &actor())
                    .await
                    .unwrap()
                    .status,
                GovernedProviderStatus::ReconciliationRequired { .. }
            ));
        }
        let status = f
            .driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status;
        assert!(matches!(
            status,
            GovernedProviderStatus::InFlight { .. }
                | GovernedProviderStatus::ReconciliationRequired { .. }
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
                .active_attempts,
            1
        );
    }
}
#[tokio::test]
async fn result_write_interruption_recovers_only_persisted_evidence() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let faults = Arc::new(FaultStore::new(state));
        let provider = Arc::new(Counting::new(Mode::Success));
        let f = fixture(faults.clone(), provider.clone(), false, config()).await;
        faults
            .fail_next(
                KeyKind::Custom(RESULT_KIND.into()),
                WriteOperation::CheckAndSet,
                timing,
            )
            .unwrap();
        let observed = f
            .driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap();
        match timing {
            FaultTiming::Before => assert!(matches!(
                observed.status,
                GovernedProviderStatus::InFlight { .. }
            )),
            FaultTiming::After => completed(&observed.status),
        }
        let repeated = f
            .driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        if matches!(timing, FaultTiming::After) {
            completed(&repeated.status);
        }
    }
}
#[tokio::test]
async fn outcome_settlement_ack_loss_is_repaired_without_invoking_again() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(state));
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(fixture(faults.clone(), provider.clone(), false, config()).await);
    let task = {
        let f = f.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            f.driver(provider, None)
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
        })
    };
    provider.entered.notified().await;
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    provider.release.add_permits(1);
    completed(&task.await.unwrap().unwrap().status);
    completed(
        &f.driver(provider.clone(), None)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        0
    );
}
#[tokio::test]
async fn known_rejection_retries_spend_new_units_and_preserve_results() {
    let provider = Arc::new(Counting::new(Mode::RejectFirst));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        true,
        config(),
    )
    .await;
    let result = f
        .driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    completed(&result.status);
    assert_eq!(result.attempts, 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .spent_units,
        2
    );
}
#[tokio::test]
async fn changed_input_actor_and_binding_are_refused_before_any_effect() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let mut action = f.action.clone();
    action.payload = serde_json::json!({"changed":true});
    assert!(
        f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &action, &actor())
            .await
            .is_err()
    );
    assert!(
        f.driver(provider.clone(), None)
            .execute(
                &f.reference,
                &references(),
                &f.action,
                &PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap()
            )
            .await
            .is_err()
    );
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let changed = BoundProvider::new_trusted(
        provider.clone(),
        &endpoint(),
        "work",
        "definition-v2",
        vec![],
    )
    .unwrap();
    let driver = GovernedProviderExecutor::new(
        f.state.clone(),
        f.coordinator.clone(),
        f.contexts.clone(),
        changed,
        config(),
        f.clock.clone(),
        None,
    )
    .unwrap();
    assert!(matches!(
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await,
        Err(GovernedProviderError::Conflict)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn encrypted_receipts_and_digest_pins_detect_result_tampering() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let encryptor = Arc::new(PayloadEncryptor::new(
        parse_master_key(&"01".repeat(32)).unwrap(),
    ));
    completed(
        &f.driver(provider.clone(), Some(encryptor.clone()))
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let operation = f
        .state
        .get(&key(
            &f,
            OPERATION_KIND,
            f.reference.execution_id().to_string(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(!operation.contains("request-secret"));
    let output_key = key(&f, RESULT_KIND, attempt(f.reference.execution_id(), 0));
    let raw = f.state.get(&output_key).await.unwrap().unwrap();
    assert!(!raw.contains("receipt-secret"));
    let mut value = encryptor.decrypt_json(&raw).unwrap();
    value["outcome"]["Executed"]["body"]["secret"] = serde_json::json!("corrupt");
    serde_json::from_value::<ActionOutcome>(value["outcome"].clone()).unwrap();
    f.state
        .set(&output_key, &encryptor.encrypt_json(&value).unwrap(), None)
        .await
        .unwrap();
    assert!(
        f.driver(provider.clone(), Some(encryptor))
            .inspect(&f.reference, &actor())
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn completed_results_remain_inspectable_after_expiry_and_revocation() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    f.coordinator
        .change(
            "revoke",
            AuthorityChange::RevokePermit {
                permit_id: "permit".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    f.clock.advance_to(Duration::from_secs(20)).unwrap();
    completed(
        &f.driver(provider.clone(), None)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replacement_preserves_backoff_and_rechecks_revocation_before_retry() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    for revoked in [false, true] {
        let provider = Arc::new(Counting::new(Mode::RejectFirst));
        let mut settings = config();
        settings.retry_strategy = RetryStrategy::Constant {
            delay: Duration::from_secs(2),
        };
        let f = fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            true,
            settings,
        )
        .await;
        let refs = references();
        let principal = actor();
        let driver = f.driver(provider.clone(), None);
        let mut first = Box::pin(driver.execute(&f.reference, &refs, &f.action, &principal));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(first.as_mut().poll(&mut cx), Poll::Pending));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            driver
                .inspect(&f.reference, &principal)
                .await
                .unwrap()
                .unwrap()
                .status,
            GovernedProviderStatus::AwaitingRetry {
                not_before_ms: 2100
            }
        ));
        drop(first);
        let replacement = f.driver(provider.clone(), None);
        let mut resumed = Box::pin(replacement.execute(&f.reference, &refs, &f.action, &principal));
        assert!(matches!(resumed.as_mut().poll(&mut cx), Poll::Pending));
        f.clock.advance_to(Duration::from_millis(1999)).unwrap();
        assert!(matches!(resumed.as_mut().poll(&mut cx), Poll::Pending));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        if revoked {
            f.coordinator
                .change(
                    "stop-retry",
                    AuthorityChange::RevokePermit {
                        permit_id: "permit".into(),
                        expected_revision: 1,
                    },
                    "issuer",
                    "stop",
                )
                .await
                .unwrap();
        }
        f.clock.advance_to(Duration::from_secs(2)).unwrap();
        let result = resumed.await;
        if revoked {
            assert!(matches!(result, Err(GovernedProviderError::Admission(_))));
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        } else {
            completed(&result.unwrap().status);
            assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        }
    }
}

#[tokio::test]
async fn lost_registration_ack_never_issues_a_replacement_send() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(faults.clone(), provider.clone(), false, config()).await;
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert!(matches!(
        f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        1
    );
}

struct HttpProvider {
    client: reqwest::Client,
    url: String,
}
#[async_trait]
impl DynProvider for HttpProvider {
    fn name(&self) -> &'static str {
        "selected"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let response = self
            .client
            .post(&self.url)
            .json(action)
            .send()
            .await
            .map_err(|_| ProviderError::Connection("HTTP response unavailable".into()))?;
        let body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| ProviderError::Connection("HTTP result unavailable".into()))?;
        Ok(ProviderResponse::success(body))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[tokio::test]
async fn real_http_result_is_recovered_without_a_second_network_invocation() {
    use axum::{Json, Router, routing::post};
    let received = Arc::new(Mutex::new(Vec::<Action>::new()));
    let handler_received = received.clone();
    let router = Router::new().route(
        "/execute",
        post(move |Json(action): Json<Action>| {
            let received = handler_received.clone();
            async move {
                received.lock().unwrap().push(action.clone());
                Json(serde_json::json!({"receipt":"network-receipt","action_id":action.id}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let provider = Arc::new(HttpProvider {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        url: format!("http://{address}/execute"),
    });
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let selected: Arc<dyn DynProvider> = provider.clone();
    let catalog = acteon_executor::catalog::QualifiedProviderCatalog::new_trusted(vec![bound(
        selected.clone(),
        false,
    )])
    .unwrap();
    let replacement: Arc<dyn DynProvider> = Arc::new(HttpProvider {
        client: reqwest::Client::new(),
        url: provider.url.clone(),
    });
    assert!(catalog.resolve(&f.action, &replacement).is_err());
    assert_eq!(f.action.provider.as_str(), "original");
    let qualified = catalog.resolve(&f.action, &selected).unwrap().clone();
    let driver = GovernedProviderExecutor::new(
        f.state.clone(),
        f.coordinator.clone(),
        f.contexts.clone(),
        qualified,
        f.settings.clone(),
        f.clock.clone(),
        None,
    )
    .unwrap();
    let first = driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let recovered = f
        .driver(provider, None)
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    completed(&first.status);
    completed(&recovered.status);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(recovered).unwrap()
    );
    {
        let actual = received.lock().unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].id, f.action.id);
        assert_eq!(actual[0].payload, f.action.payload);
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn evidence_is_immutable_and_repeated_settlement_releases_capacity_once() {
    use acteon_governance::{AttemptEvidenceReference, AttemptStatus};
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    completed(
        &f.driver(provider, None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let id = attempt(f.reference.execution_id(), 0);
    let start = &snapshot.starts[&id];
    let evidence = start.evidence.clone().unwrap();
    f.coordinator
        .settle_with_evidence(&id, &start.token, AttemptStatus::Settled, evidence.clone())
        .await
        .unwrap();
    assert!(
        f.coordinator
            .settle_with_evidence(
                &id,
                &start.token,
                AttemptStatus::Settled,
                AttemptEvidenceReference {
                    id: evidence.id,
                    digest: "f".repeat(64)
                }
            )
            .await
            .is_err()
    );
    let root =
        &f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()];
    assert_eq!(root.active_attempts, 0);
    assert_eq!(root.spent_units, 1);
}

#[tokio::test]
async fn historical_observation_cannot_authorize_an_expired_effect() {
    use acteon_governance::permit::PermittedAttempt;
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    f.clock.advance_to(Duration::from_secs(20)).unwrap();
    let historical = f
        .contexts
        .recover_reference_for_observation(&f.reference)
        .await
        .unwrap();
    assert!(
        f.contexts
            .recover_reference(&f.reference, 20_100)
            .await
            .is_err()
    );
    assert!(
        f.coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "expired-attempt",
                context: &historical,
                permits: &references(),
                effect: bound(provider.clone(), false).effect(),
                request_digest: f.reference.request_digest(),
                units: 1,
                clock: f.clock.as_ref(),
            })
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against real Redis"]
async fn independent_redis_workers_observe_one_provider_attempt() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let settings = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("governed-provider-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let state: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
    let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
    independent_provider_contract(state, peer).await;
}

#[allow(clippy::too_many_lines)] // Keep the ordered multi-client contract in one test scenario.
async fn independent_provider_contract(state: Arc<dyn StateStore>, peer: Arc<dyn StateStore>) {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    use acteon_governance::context::CONTEXT_KIND;
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(fixture(state.clone(), provider.clone(), false, config()).await);
    let coordinator = AuthorityCoordinator::connect(peer.clone(), "city", "tenant")
        .await
        .unwrap();
    let contexts = Arc::new(
        TrustedContextStore::new(
            peer.clone(),
            coordinator.clone(),
            "domain".into(),
            "k1".into(),
            vec![ContextSigningKey::new("k1".into(), vec![1; 32]).unwrap()],
        )
        .unwrap(),
    );
    let replacement = GovernedProviderExecutor::new(
        peer.clone(),
        coordinator,
        contexts,
        bound(provider.clone(), false),
        config(),
        f.clock.clone(),
        None,
    )
    .unwrap();
    let task = {
        let f = f.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            f.driver(provider, None)
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .unwrap();
    assert!(matches!(
        replacement
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::InFlight { .. }
    ));
    let id = attempt(f.reference.execution_id(), 0);
    let initial = f.coordinator.snapshot().await.unwrap();
    let seal = initial.starts[&id].operation_evidence.clone().unwrap();
    assert_eq!(seal.id, f.reference.execution_id().to_string());
    let envelope = peer
        .get(&key(&f, OPERATION_KIND, seal.id.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        seal.digest,
        format!("{:x}", Sha256::digest(envelope.as_bytes()))
    );
    let observer = replacement
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    let pending = observer
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    f.coordinator
        .change(
            "cancel",
            AuthorityChange::CancelExecution {
                execution_id: f.reference.execution_id().to_string(),
            },
            "host",
            "stop further work",
        )
        .await
        .unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(
                serde_json::json!({"external_receipt":"committed"}),
            ),
        },
    );
    completed(
        &observer
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    provider.release.add_permits(1);
    completed(
        &tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .status,
    );
    // Expire authority only after the late worker has returned. Advancing the
    // manual clock while it is blocked also expires its provider timeout;
    // remote backend awaits can then return the earlier uncertain receipt.
    // Historical replay must still preserve the accepted finality after expiry.
    f.clock.advance_to(Duration::from_secs(20)).unwrap();
    completed(
        &observer
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    let restarted_coordinator = AuthorityCoordinator::connect(peer.clone(), "city", "tenant")
        .await
        .unwrap();
    let restarted_contexts = Arc::new(
        TrustedContextStore::new(
            peer.clone(),
            restarted_coordinator.clone(),
            "domain".into(),
            "k1".into(),
            vec![ContextSigningKey::new("k1".into(), vec![1; 32]).unwrap()],
        )
        .unwrap(),
    );
    let restarted = GovernedProviderExecutor::new(
        peer.clone(),
        restarted_coordinator.clone(),
        restarted_contexts,
        bound(provider.clone(), false),
        config(),
        f.clock.clone(),
        None,
    )
    .unwrap();
    completed(
        &restarted
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    let record = restarted
        .reconciliation_record(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.attempt_id, id);
    assert!(record.original_evidence.is_none());
    let snapshot = restarted_coordinator.snapshot().await.unwrap();
    let root = &snapshot.roots[&f.reference.execution_id().to_string()];
    assert!(root.cancelled);
    assert_eq!((root.spent_units, root.active_attempts), (1, 0));
    assert_eq!(snapshot.starts[&id].operation_evidence, Some(seal));
    assert!(snapshot.starts[&id].evidence.is_none()); // late worker cannot replace finality
    let archive = HistoricalProviderStore::new(
        peer.clone(),
        restarted_coordinator.clone(),
        f.contexts.clone(),
        None,
    );
    let historical = archive
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    completed(&historical.receipt.status);
    assert_eq!(historical.operation_integrity, OperationIntegrity::Sealed);
    assert!(historical.cancellation_fenced);
    assert_eq!(
        historical.metadata.unwrap().original_action_id,
        f.action.id.to_string()
    );
    assert!(historical.attempts[0].reconciliation.is_some());
    // A separate client cannot rewrite original delivery identity behind the reader.
    let operation_key = key(&f, OPERATION_KIND, f.reference.execution_id().to_string());
    let mut modified: serde_json::Value = serde_json::from_str(&envelope).unwrap();
    modified["action"]["id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
    peer.set(
        &operation_key,
        &serde_json::to_string(&modified).unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        restarted.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Conflict)
    ));
    peer.set(&operation_key, &envelope, None).await.unwrap();
    completed(
        &restarted
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    for (kind, id) in [
        (COORDINATOR_KIND, "authority".to_string()),
        (CONTEXT_KIND, f.reference.context_id().to_string()),
        (OPERATION_KIND, f.reference.execution_id().to_string()),
        (RESULT_KIND, attempt(f.reference.execution_id(), 0)),
        (RECONCILIATION_KIND, attempt(f.reference.execution_id(), 0)),
    ] {
        assert!(peer.delete(&key(&f, kind, id)).await.unwrap());
    }
}

#[tokio::test]
async fn independent_memory_workers_observe_one_provider_attempt() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    independent_provider_contract(state.clone(), state).await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL; independent PostgreSQL clients"]
async fn independent_postgres_workers_observe_one_provider_attempt() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").expect("PostgreSQL URL required"),
        table_prefix: format!("provider_contract_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    independent_provider_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
    )
    .await;
    let pool = sqlx::PgPool::connect(&config.url).await.unwrap();
    // Only the tables created for this UUID-scoped test fixture.
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!(
            "DROP TABLE public.{}{suffix}",
            config.table_prefix
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

#[cfg(feature = "dynamodb")]
#[tokio::test]
#[ignore = "requires DYNAMODB_ENDPOINT; independent DynamoDB Local clients"]
async fn independent_dynamodb_workers_observe_one_provider_attempt() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(
            std::env::var("DYNAMODB_ENDPOINT").expect("DynamoDB Local endpoint required"),
        ),
        table_name: format!("provider_contract_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("provider_contract_{}", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    independent_provider_contract(
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
        Arc::new(DynamoStateStore::new(&config).await.unwrap()),
    )
    .await;
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}

async fn bind_credential(f: &mut Fixture, provider: Arc<dyn DynProvider>) {
    use acteon_governance::credential::{CredentialAuthority, CredentialReference};
    let effects = vec![bound(provider, f.known).effect().clone()];
    let budget = RootBudgetLimits {
        max_units: 4,
        max_concurrent: 2,
        deadline_ms: 10_000,
    };
    let issuance = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("issuer", PrincipalKind::Human).unwrap(),
        subjects: vec![actor()],
        effects: effects.clone(),
        valid_from_ms: 0,
        limits: budget.clone(),
    };
    f.coordinator
        .publish_credential(
            "credential-issue",
            CredentialAuthority {
                ceiling: ExecutionPermit {
                    id: "key".into(),
                    revision: 1,
                    subject: actor(),
                    effects: effects.clone(),
                    valid_from_ms: 0,
                    limits: budget.clone(),
                },
                auth_method: "api_key".into(),
                execution_enabled: true,
            },
            0,
            &issuance,
            &f.coordinator.snapshot().await.unwrap().stamp(),
            "reviewed",
            100,
        )
        .await
        .unwrap();
    let context = f
        .contexts
        .capture_credentialed_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: governed_provider_input_digest(&f.action).unwrap(),
                },
                credential_id: "key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: "placeholder".into(),
                accepted_effects: effects,
                deadline_ms: 10_000,
                evaluated_authority: f.coordinator.snapshot().await.unwrap().stamp(),
            },
            &references(),
            CredentialReference {
                id: "key".into(),
                accepted_revision: 1,
            },
            budget,
            f.clock.as_ref(),
        )
        .await
        .unwrap();
    f.reference = context.reference().unwrap();
}

#[tokio::test]
async fn credential_revocation_during_durable_backoff_blocks_the_real_provider_retry() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let provider = Arc::new(Counting::new(Mode::RejectFirst));
    let mut settings = config();
    settings.retry_strategy = RetryStrategy::Constant {
        delay: Duration::from_secs(2),
    };
    let mut f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        true,
        settings,
    )
    .await;
    bind_credential(&mut f, provider.clone()).await;
    let driver = f
        .driver(provider.clone(), None)
        .require_credential_authority();
    let refs = references();
    let principal = actor();
    let mut execution = Box::pin(driver.execute(&f.reference, &refs, &f.action, &principal));
    assert!(matches!(
        execution
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    f.coordinator
        .change(
            "credential-revoke",
            AuthorityChange::RevokeCredential {
                credential_id: "key".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    f.clock.advance_to(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        execution.await,
        Err(GovernedProviderError::Admission(_))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .spent_units,
        1
    );
}

#[tokio::test]
async fn required_credential_profile_refuses_actor_only_contexts() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .require_credential_authority();
    assert!(matches!(
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await,
        Err(GovernedProviderError::Admission(
            "CREDENTIAL_AUTHORITY_REQUIRED"
        ))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
    assert!(
        f.state
            .get(&key(
                &f,
                OPERATION_KIND,
                f.reference.execution_id().to_string()
            ))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn an_accepted_root_cannot_adopt_a_new_binding_or_failure_contract() {
    use acteon_executor::catalog::QualifiedProviderCatalog;
    for change_failure_contract in [false, true] {
        let provider = Arc::new(Counting::new(Mode::Success));
        let f = fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await;
        let selected: Arc<dyn DynProvider> = provider.clone();
        let changed = if change_failure_contract {
            bound(selected.clone(), false)
                .with_failure_contract(Arc::new(KnownRateLimit))
                .unwrap()
        } else {
            let new_binding = BoundProvider::new_trusted(
                selected.clone(),
                &endpoint(),
                "work",
                "definition-v2",
                vec![],
            )
            .unwrap();
            let catalog = QualifiedProviderCatalog::new_trusted(vec![new_binding]).unwrap();
            catalog.resolve(&f.action, &selected).unwrap().clone()
        };
        assert_ne!(changed.effect(), bound(selected, false).effect());
        let driver = GovernedProviderExecutor::new(
            f.state.clone(),
            f.coordinator.clone(),
            f.contexts.clone(),
            changed,
            f.settings.clone(),
            f.clock.clone(),
            None,
        )
        .unwrap();
        assert!(matches!(
            driver
                .execute(&f.reference, &references(), &f.action, &actor())
                .await,
            Err(GovernedProviderError::Admission("ATTEMPT_DENIED"))
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let snapshot = f.coordinator.snapshot().await.unwrap();
        let root = &snapshot.roots[&f.reference.execution_id().to_string()];
        assert_eq!(root.spent_units, 0);
        assert_eq!(root.active_attempts, 0);
    }
}

async fn mediated(
    mediator: &GovernedProviderMediator,
    action: &Action,
    selected: &Arc<dyn DynProvider>,
    authority: Option<&ProviderExecutionAuthority>,
) -> ActionOutcome {
    mediator
        .execute(ProviderInvocation {
            action,
            selected,
            authority,
            context: None,
            origin: ProviderInvocationOrigin::Dispatch,
        })
        .await
}
fn mediation_code(outcome: ActionOutcome) -> String {
    let ActionOutcome::Failed(error) = outcome else {
        panic!("expected refusal")
    };
    assert!(!error.retryable);
    error.code
}

#[tokio::test]
async fn strict_mediator_requires_authority_exact_instance_actor_and_credential() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let selected: Arc<dyn DynProvider> = provider.clone();
    let mut f = fixture(
        Arc::new(MemoryStateStore::new()),
        selected.clone(),
        false,
        config(),
    )
    .await;
    let mediator = GovernedProviderMediator::new(vec![f.driver(selected.clone(), None)]).unwrap();
    assert_eq!(
        mediation_code(mediated(&mediator, &f.action, &selected, None).await),
        "EXECUTION_AUTHORITY_REQUIRED"
    );
    // Even a signed actor-only context cannot enter the credential profile.
    let actor_only =
        ProviderExecutionAuthority::new_trusted(f.reference.clone(), actor(), references());
    assert_eq!(
        mediation_code(mediated(&mediator, &f.action, &selected, Some(&actor_only)).await),
        "GOVERNED_EXECUTION_REFUSED"
    );
    bind_credential(&mut f, selected.clone()).await;
    let authority =
        ProviderExecutionAuthority::new_trusted(f.reference.clone(), actor(), references());
    let substitute: Arc<dyn DynProvider> = Arc::new(Counting::new(Mode::Success));
    assert_eq!(
        mediation_code(mediated(&mediator, &f.action, &substitute, Some(&authority)).await),
        "PROVIDER_UNQUALIFIED"
    );
    let wrong_actor = ProviderExecutionAuthority::new_trusted(
        f.reference.clone(),
        PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap(),
        references(),
    );
    assert_eq!(
        mediation_code(mediated(&mediator, &f.action, &selected, Some(&wrong_actor)).await),
        "GOVERNED_EXECUTION_REFUSED"
    );
    let mut altered = f.action.clone();
    altered.payload = serde_json::json!({"different":"work"});
    assert_eq!(
        mediation_code(mediated(&mediator, &altered, &selected, Some(&authority)).await),
        "GOVERNED_EXECUTION_REFUSED"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        mediation_code(
            mediator
                .execute(ProviderInvocation {
                    action: &f.action,
                    selected: &selected,
                    authority: Some(&authority),
                    context: Some(&acteon_provider::DispatchContext::default()),
                    origin: ProviderInvocationOrigin::Dispatch,
                })
                .await
        ),
        "GOVERNED_CONTEXT_UNSUPPORTED"
    );
    for _ in 0..2 {
        assert!(matches!(
            mediated(&mediator, &f.action, &selected, Some(&authority)).await,
            ActionOutcome::Executed(_)
        ));
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn strict_mediator_preserves_uncertain_work_without_automatic_resend() {
    let provider = Arc::new(Counting::new(Mode::Connection));
    let selected: Arc<dyn DynProvider> = provider.clone();
    let mut f = fixture(
        Arc::new(MemoryStateStore::new()),
        selected.clone(),
        false,
        config(),
    )
    .await;
    bind_credential(&mut f, selected.clone()).await;
    let authority =
        ProviderExecutionAuthority::new_trusted(f.reference.clone(), actor(), references());
    let mediator = GovernedProviderMediator::new(vec![f.driver(selected.clone(), None)]).unwrap();
    for _ in 0..2 {
        let outcome = mediated(&mediator, &f.action, &selected, Some(&authority)).await;
        let ActionOutcome::ProviderPending(work) = outcome else {
            panic!("uncertain work must remain pending")
        };
        assert_eq!(work.execution_id, f.reference.execution_id());
        assert_eq!(work.attempts, 1);
        assert!(matches!(
            work.state,
            acteon_core::ProviderWorkState::ReconciliationRequired { .. }
        ));
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let root = &snapshot.roots[&f.reference.execution_id().to_string()];
    assert_eq!(root.active_attempts, 1);
    assert_eq!(root.spent_units, 1);
}

#[tokio::test]
async fn strict_mediator_rejects_ambiguous_routes() {
    let provider: Arc<dyn DynProvider> = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    assert!(GovernedProviderMediator::new(vec![]).is_err());
    assert!(
        GovernedProviderMediator::new(vec![
            f.driver(provider.clone(), None),
            f.driver(provider, None)
        ])
        .is_err()
    );
}

#[tokio::test]
async fn strict_mediator_observes_current_revocation_before_any_attempt() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let selected: Arc<dyn DynProvider> = provider.clone();
    let mut f = fixture(
        Arc::new(MemoryStateStore::new()),
        selected.clone(),
        false,
        config(),
    )
    .await;
    bind_credential(&mut f, selected.clone()).await;
    let authority =
        ProviderExecutionAuthority::new_trusted(f.reference.clone(), actor(), references());
    let mediator = GovernedProviderMediator::new(vec![f.driver(selected.clone(), None)]).unwrap();
    f.coordinator
        .change(
            "stop-credential",
            AuthorityChange::RevokeCredential {
                credential_id: "key".into(),
                expected_revision: 1,
            },
            "issuer",
            "offboarding",
        )
        .await
        .unwrap();
    assert_eq!(
        mediation_code(mediated(&mediator, &f.action, &selected, Some(&authority)).await),
        "GOVERNED_EXECUTION_REFUSED"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    assert!(snapshot.starts.is_empty());
    assert_eq!(
        snapshot.roots[&f.reference.execution_id().to_string()].spent_units,
        0
    );
}

fn finality_verifier()
-> Arc<dyn acteon_executor::governed::reconciliation::ProviderReconciliationVerifier> {
    use acteon_executor::governed::reconciliation::HmacFinalityVerifier;
    Arc::new(
        HmacFinalityVerifier::new_trusted(
            "provider-finality-v1",
            std::collections::BTreeMap::from([("issuer-key".into(), vec![47; 32])]),
        )
        .unwrap(),
    )
}
fn finality_proof(
    attempt: acteon_executor::governed::reconciliation::ReconciliationAttempt,
    finality: acteon_executor::governed::reconciliation::ProviderFinality,
) -> Vec<u8> {
    acteon_executor::governed::reconciliation::sign_finality_receipt(
        attempt,
        finality,
        "provider-finality-v1",
        "issuer-key",
        &[47; 32],
    )
    .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One ordered recovery and accounting scenario.
async fn reconciliation_preserves_uncertainty_and_recovers_after_cancellation_and_expiry() {
    use acteon_executor::governed::reconciliation::ProviderFinality;
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    assert!(matches!(
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::ReconciliationRequired { .. }
    ));
    let original = f.coordinator.snapshot().await.unwrap();
    let id = attempt(f.reference.execution_id(), 0);
    let original_ref = original.starts[&id].evidence.clone().unwrap();
    let original_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(RESULT_KIND.into()),
        id.clone(),
    );
    let original_bytes = f.state.get(&original_key).await.unwrap().unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    f.coordinator
        .change(
            "cancel",
            AuthorityChange::CancelExecution {
                execution_id: f.reference.execution_id().to_string(),
            },
            "host",
            "stop execution",
        )
        .await
        .unwrap();
    f.clock.advance_to(Duration::from_secs(20)).unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(
                serde_json::json!({"external_receipt":"committed"}),
            ),
        },
    );
    completed(
        &driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    completed(
        &driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    let restarted = f.driver(provider.clone(), None); // no current verifier needed for pinned evidence
    completed(
        &restarted
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    completed(
        &restarted
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let start = &snapshot.starts[&id];
    assert_eq!(start.evidence, Some(original_ref.clone()));
    assert_eq!(
        start.reconciliation.as_ref().unwrap().original_evidence,
        Some(original_ref)
    );
    assert_eq!(
        f.state.get(&original_key).await.unwrap().unwrap(),
        original_bytes
    );
    assert_eq!(
        (
            snapshot.roots[&f.reference.execution_id().to_string()].spent_units,
            snapshot.roots[&f.reference.execution_id().to_string()].active_attempts
        ),
        (1, 0)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconciliation_no_effect_is_terminal_and_not_an_automatic_retry_permit() {
    use acteon_executor::governed::reconciliation::ProviderFinality;
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = finality_proof(
        pending.clone(),
        ProviderFinality::NoEffect {
            reason: "provider fenced the attempt without an effect".into(),
        },
    );
    let receipt = driver
        .reconcile(&f.reference, &actor(), &proof)
        .await
        .unwrap();
    let GovernedProviderStatus::Completed {
        outcome: ActionOutcome::Failed(error),
    } = receipt.status
    else {
        panic!("no effect should be a terminal failure")
    };
    assert_eq!(error.code, "RECONCILED_NO_EFFECT");
    assert!(!error.retryable);
    assert_eq!(error.attempts, 1);
    assert_eq!(
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .attempts,
        1
    );
    let conflicting = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"different":true})),
        },
    );
    assert!(
        driver
            .reconcile(&f.reference, &actor(), &conflicting)
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        0
    );
}

#[tokio::test]
async fn reconciliation_refuses_wrong_signature_attempt_binding_and_oversized_claims() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, sign_finality_receipt};
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let result = || ProviderFinality::Completed {
        response: ProviderResponse::success(serde_json::json!({"done":true})),
    };
    let mut wrong_attempt = pending.clone();
    wrong_attempt.token = uuid::Uuid::new_v4().to_string();
    let mut wrong_input = pending.clone();
    wrong_input.binding_digest = "f".repeat(64);
    let mut wrong_action = pending.clone();
    wrong_action.action_id = uuid::Uuid::new_v4().to_string();
    let proof = finality_proof(pending.clone(), result());
    let mut tampered: serde_json::Value = serde_json::from_slice(&proof).unwrap();
    tampered["finality"]["response"]["body"] = serde_json::json!({"forged":true});
    for proof in [
        sign_finality_receipt(
            pending,
            result(),
            "provider-finality-v1",
            "issuer-key",
            &[48; 32],
        )
        .unwrap(),
        finality_proof(wrong_attempt, result()),
        finality_proof(wrong_input, result()),
        finality_proof(wrong_action, result()),
        serde_json::to_vec(&tampered).unwrap(),
        vec![b'x'; 64 * 1024 + 1],
        b"model said completed".to_vec(),
    ] {
        assert!(
            driver
                .reconcile(&f.reference, &actor(), &proof)
                .await
                .is_err()
        );
    }
    let snapshot = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&f.reference.execution_id().to_string()].active_attempts,
        1
    );
    assert!(snapshot.starts.values().all(|s| s.reconciliation.is_none()));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconciliation_repairs_proof_and_settlement_ack_loss_without_resending() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    for kind in [RECONCILIATION_KIND, COORDINATOR_KIND] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let provider = Arc::new(Counting::new(Mode::Connection));
        let f = fixture(faults.clone(), provider.clone(), false, config()).await;
        let driver = f
            .driver(provider.clone(), None)
            .with_trusted_reconciliation_verifier(finality_verifier())
            .unwrap();
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap();
        let pending = driver
            .reconciliation_attempt(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap();
        let proof = finality_proof(
            pending,
            ProviderFinality::Completed {
                response: ProviderResponse::success(serde_json::json!({"done":true})),
            },
        );
        faults
            .fail_next(
                KeyKind::Custom(kind.into()),
                if kind == RECONCILIATION_KIND {
                    WriteOperation::CheckAndSet
                } else {
                    WriteOperation::CompareAndSwap
                },
                FaultTiming::After,
            )
            .unwrap();
        assert!(
            driver
                .reconcile(&f.reference, &actor(), &proof)
                .await
                .is_err()
        );
        let bare = f.driver(provider.clone(), None);
        if kind == RECONCILIATION_KIND {
            assert!(matches!(
                bare.inspect(&f.reference, &actor())
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                GovernedProviderStatus::ReconciliationRequired { .. }
            ));
            assert_eq!(
                f.coordinator.snapshot().await.unwrap().roots
                    [&f.reference.execution_id().to_string()]
                    .active_attempts,
                1
            );
        }
        let restarted = f
            .driver(provider.clone(), None)
            .with_trusted_reconciliation_verifier(finality_verifier())
            .unwrap();
        completed(
            &restarted
                .inspect(&f.reference, &actor())
                .await
                .unwrap()
                .unwrap()
                .status,
        );
        completed(
            &restarted
                .reconcile(&f.reference, &actor(), &proof)
                .await
                .unwrap()
                .status,
        );
        assert_eq!(
            f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
                .active_attempts,
            0
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn reconciliation_reverifies_unpinned_evidence_and_digest_pins_accepted_history() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(RECONCILIATION_KIND.into()),
        pending.attempt_id.clone(),
    );
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"done":true})),
        },
    );
    faults
        .fail_next(
            KeyKind::Custom(RECONCILIATION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    let original = f.state.get(&key).await.unwrap().unwrap();
    let mut tampered: serde_json::Value = serde_json::from_str(&original).unwrap();
    tampered["outcome"]["Executed"]["body"] = serde_json::json!({"forged":true});
    f.state
        .set(&key, &serde_json::to_string(&tampered).unwrap(), None)
        .await
        .unwrap();
    assert!(driver.inspect(&f.reference, &actor()).await.is_err());
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        1
    );
    f.state.set(&key, &original, None).await.unwrap();
    completed(
        &driver
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
    );
    f.state
        .set(&key, &serde_json::to_string(&tampered).unwrap(), None)
        .await
        .unwrap();
    assert!(
        f.driver(provider.clone(), None)
            .inspect(&f.reference, &actor())
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconciliation_of_inflight_work_fences_late_worker_settlement_without_reopening() {
    use acteon_executor::governed::reconciliation::ProviderFinality;
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(
        fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await,
    );
    let running_f = f.clone();
    let running_provider = provider.clone();
    let running = tokio::spawn(async move {
        running_f
            .driver(running_provider, None)
            .execute(
                &running_f.reference,
                &references(),
                &running_f.action,
                &actor(),
            )
            .await
    });
    provider.entered.notified().await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"external_finality":true})),
        },
    );
    completed(
        &driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    provider.release.add_permits(1);
    completed(&running.await.unwrap().unwrap().status);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let start = snapshot.starts.values().next().unwrap();
    assert!(start.evidence.is_none());
    assert!(
        start
            .reconciliation
            .as_ref()
            .unwrap()
            .original_evidence
            .is_none()
    );
    assert_eq!(
        snapshot.roots[&f.reference.execution_id().to_string()].active_attempts,
        0
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconciliation_of_abandoned_registration_requires_finality_and_never_sends() {
    use acteon_executor::governed::reconciliation::ProviderFinality;
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(faults.clone(), provider.clone(), false, config()).await;
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    assert!(matches!(
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    // The external issuer attests a permanent no-effect tombstone, not an empty
    // lookup or an elapsed lease. Lost acknowledgement alone is insufficient.
    let proof = finality_proof(
        pending,
        ProviderFinality::NoEffect {
            reason: "external attempt tombstone is irrevocable".into(),
        },
    );
    assert!(
        driver
            .reconcile(
                &f.reference,
                &PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap(),
                &proof
            )
            .await
            .is_err()
    );
    assert!(
        f.driver(provider.clone(), None)
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        1
    );
    let receipt = driver
        .reconcile(&f.reference, &actor(), &proof)
        .await
        .unwrap();
    assert!(matches!(
        receipt.status,
        GovernedProviderStatus::Completed {
            outcome: ActionOutcome::Failed(_)
        }
    ));
    let record = driver
        .reconciliation_record(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(record.original_evidence.is_none());
    assert_eq!(record.verifier_revision, "provider-finality-v1");
    assert_eq!(record.proof_digest.len(), 64);
    assert!(matches!(
        f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::Completed {
            outcome: ActionOutcome::Failed(_)
        }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reconciliation_candidate_cannot_replace_a_known_result_awaiting_ledger_ack() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(fixture(faults.clone(), provider.clone(), false, config()).await);
    let work = f.clone();
    let selected = provider.clone();
    let running = tokio::spawn(async move {
        work.driver(selected, None)
            .execute(&work.reference, &references(), &work.action, &actor())
            .await
    });
    provider.entered.notified().await;
    let observer = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    let pending = observer
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"external_receipt":true})),
        },
    );
    faults
        .fail_next(
            KeyKind::Custom(RECONCILIATION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        observer
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    let consumed = faults.consumed();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() == consumed {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let receipt = observer
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let GovernedProviderStatus::Completed {
        outcome: ActionOutcome::Executed(response),
    } = receipt.status
    else {
        panic!("ordinary receipt must win")
    };
    assert_eq!(response.body["secret"], "receipt-secret");
    assert!(
        observer
            .reconciliation_record(&f.reference, &actor())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        observer
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    release.send(()).unwrap();
    completed(&running.await.unwrap().unwrap().status);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .active_attempts,
        0
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn original_operation_seal_detects_delivery_identity_tampering_in_plain_and_encrypted_state()
{
    for encrypted in [false, true] {
        let provider = Arc::new(Counting::new(Mode::Success));
        let f = fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await;
        let encryptor = encrypted.then(|| {
            Arc::new(PayloadEncryptor::new(
                parse_master_key(&"01".repeat(32)).unwrap(),
            ))
        });
        let driver = f.driver(provider.clone(), encryptor.clone());
        completed(
            &driver
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
                .unwrap()
                .status,
        );
        let operation_key = key(&f, OPERATION_KIND, f.reference.execution_id().to_string());
        let original = f.state.get(&operation_key).await.unwrap().unwrap();
        let raw = encryptor
            .as_ref()
            .map_or_else(|| original.clone(), |e| e.decrypt_str(&original).unwrap());
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["action"]["id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
        let changed: Action = serde_json::from_value(value["action"].clone()).unwrap();
        assert_eq!(
            governed_provider_input_digest(&changed).unwrap(),
            f.reference.request_digest()
        );
        let changed = serde_json::to_string(&value).unwrap();
        let encoded = encryptor
            .as_ref()
            .map_or_else(|| changed.clone(), |e| e.encrypt_str(&changed).unwrap());
        f.state.set(&operation_key, &encoded, None).await.unwrap();
        assert!(matches!(
            driver.inspect(&f.reference, &actor()).await,
            Err(GovernedProviderError::Conflict)
        ));
        assert!(matches!(
            driver
                .execute(&f.reference, &references(), &f.action, &actor())
                .await,
            Err(GovernedProviderError::Conflict)
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        f.state.set(&operation_key, &original, None).await.unwrap();
        completed(
            &driver
                .inspect(&f.reference, &actor())
                .await
                .unwrap()
                .unwrap()
                .status,
        );
    }
}

#[tokio::test]
async fn history_survives_changed_retry_configuration_stop_and_expiry_with_encrypted_records() {
    for encrypted in [false, true] {
        let provider = Arc::new(Counting::new(Mode::Success));
        let mut f = fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await;
        let encryptor = encrypted.then(|| {
            Arc::new(PayloadEncryptor::new(
                parse_master_key(&"01".repeat(32)).unwrap(),
            ))
        });
        let driver = f.driver(provider.clone(), encryptor.clone());
        completed(
            &driver
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
                .unwrap()
                .status,
        );
        drop(driver);
        f.settings.max_retries = 0;
        assert!(matches!(
            f.driver(provider.clone(), encryptor.clone())
                .inspect(&f.reference, &actor())
                .await,
            Err(GovernedProviderError::Conflict)
        ));
        f.coordinator
            .change(
                "cancel",
                AuthorityChange::CancelExecution {
                    execution_id: f.reference.execution_id().to_string(),
                },
                "host",
                "retire execution",
            )
            .await
            .unwrap();
        f.clock.advance_to(Duration::from_secs(20)).unwrap();
        let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
        let archive = f.history(encryptor);
        let receipt = archive
            .inspect_execution(f.reference.execution_id(), &actor())
            .await
            .unwrap()
            .unwrap();
        assert_public_history_wire(&receipt);
        completed(&receipt.receipt.status);
        assert!(receipt.cancellation_fenced);
        assert_eq!(receipt.operation_integrity, OperationIntegrity::Sealed);
        assert_eq!(receipt.metadata.unwrap().max_attempts, 3);
        assert_eq!(receipt.attempts.len(), 1);
        assert_eq!(
            before,
            serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn history_preserves_unresolved_evidence_and_charges_without_a_provider_or_verifier() {
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    f.driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let before = f.coordinator.snapshot().await.unwrap();
    let receipt = f
        .history(None)
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        receipt.receipt.status,
        GovernedProviderStatus::ReconciliationRequired { .. }
    ));
    assert_eq!(
        receipt.attempts[0].original_evidence,
        before.starts[&attempt(f.reference.execution_id(), 0)].evidence
    );
    assert!(receipt.attempts[0].original_outcome.is_some());
    assert!(receipt.attempts[0].reconciliation.is_none());
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
    );
    let root = &before.roots[&f.reference.execution_id().to_string()];
    assert_eq!((root.spent_units, root.active_attempts), (1, 1));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn history_does_not_adopt_an_uncommitted_finality_proof_or_repair_state() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"done":true})),
        },
    );
    faults
        .fail_next(
            KeyKind::Custom(RECONCILIATION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    let consumed = faults.consumed();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    for _ in 0..2 {
        let receipt = f
            .history(None)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            receipt.receipt.status,
            GovernedProviderStatus::ReconciliationRequired { .. }
        ));
        assert!(receipt.attempts[0].reconciliation.is_none());
    }
    assert_eq!(faults.consumed(), consumed);
    assert_eq!(
        before,
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn history_reads_pinned_finality_and_refuses_missing_or_corrupt_attestations() {
    use acteon_executor::governed::reconciliation::{ProviderFinality, RECONCILIATION_KIND};
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let pending = driver
        .reconciliation_attempt(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = finality_proof(
        pending,
        ProviderFinality::Completed {
            response: ProviderResponse::success(serde_json::json!({"external_receipt":"done"})),
        },
    );
    completed(
        &driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .unwrap()
            .status,
    );
    drop(driver);
    let archive = f.history(None);
    let receipt = archive
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert_public_history_wire(&receipt);
    completed(&receipt.receipt.status);
    assert!(receipt.attempts[0].original_evidence.is_some());
    assert!(receipt.attempts[0].original_outcome.is_some());
    assert_eq!(
        receipt.attempts[0]
            .reconciliation
            .as_ref()
            .unwrap()
            .prior_status,
        acteon_governance::AttemptStatus::Uncertain
    );
    let storage = key(
        &f,
        RECONCILIATION_KIND,
        attempt(f.reference.execution_id(), 0),
    );
    let original = f.state.get(&storage).await.unwrap().unwrap();
    f.state.delete(&storage).await.unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Unavailable)
    ));
    f.state
        .set(&storage, &format!("{original} "), None)
        .await
        .unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Conflict)
    ));
    f.state.set(&storage, &original, None).await.unwrap();
    completed(
        &archive
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .receipt
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn legacy_history_does_not_invent_metadata_seals_or_hide_attempts_using_mutated_settings() {
    let provider = Arc::new(Counting::new(Mode::RejectFirst));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        true,
        config(),
    )
    .await;
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let storage = key(&f, COORDINATOR_KIND, "authority".into());
    let mut state: serde_json::Value =
        serde_json::from_str(&f.state.get(&storage).await.unwrap().unwrap()).unwrap();
    // Emulate retained pre-seal rows using the optional wire-field compatibility.
    for row in state["starts"].as_object_mut().unwrap().values_mut() {
        row.as_object_mut().unwrap().remove("operation_evidence");
    }
    f.state
        .set(&storage, &state.to_string(), None)
        .await
        .unwrap();
    let operation_key = key(&f, OPERATION_KIND, f.reference.execution_id().to_string());
    let mut op: serde_json::Value =
        serde_json::from_str(&f.state.get(&operation_key).await.unwrap().unwrap()).unwrap();
    op["action"]["id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
    op["settings"]["max_attempts"] = serde_json::json!(1);
    op["settings"]["delays_ns"] = serde_json::json!([]);
    f.state
        .set(&operation_key, &op.to_string(), None)
        .await
        .unwrap();
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    let receipt = f
        .history(None)
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert_public_history_wire(&receipt);
    completed(&receipt.receipt.status);
    assert_eq!(receipt.operation_integrity, OperationIntegrity::Legacy);
    assert!(receipt.metadata.is_none());
    assert_eq!(receipt.binding.unwrap().provider_revision, "definition-v1");
    assert_eq!(receipt.receipt.attempts, 2);
    assert_eq!(receipt.attempts.len(), 2);
    assert_eq!(
        before,
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn history_requires_the_authenticated_owner_scope_signed_context_and_original_operation() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let archive = f.history(None);
    let foreign = PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &foreign).await,
        Err(GovernedProviderError::Ownership)
    ));
    assert!(matches!(
        archive
            .inspect_execution(f.reference.execution_id(), &foreign)
            .await,
        Err(GovernedProviderError::Ownership)
    ));
    assert!(
        archive
            .inspect_execution(uuid::Uuid::new_v4(), &actor())
            .await
            .unwrap()
            .is_none()
    );
    let other = AuthorityCoordinator::initialize(
        f.state.clone(),
        "other-scope",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let wrong_scope =
        HistoricalProviderStore::new(f.state.clone(), other, f.contexts.clone(), None);
    assert!(matches!(
        wrong_scope.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Ownership)
    ));
    let operation_key = key(&f, OPERATION_KIND, f.reference.execution_id().to_string());
    let original = f.state.get(&operation_key).await.unwrap().unwrap();
    let mut op: serde_json::Value = serde_json::from_str(&original).unwrap();
    op["action"]["id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
    f.state
        .set(&operation_key, &op.to_string(), None)
        .await
        .unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Conflict)
    ));
    f.state.set(&operation_key, &original, None).await.unwrap();
    let context_key = key(
        &f,
        acteon_governance::context::CONTEXT_KIND,
        f.reference.context_id().to_string(),
    );
    f.state.delete(&context_key).await.unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Unavailable)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn history_does_not_promote_a_known_result_before_ledger_acknowledgement() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Block));
    let f = Arc::new(fixture(faults.clone(), provider.clone(), false, config()).await);
    let task = {
        let f = f.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            f.driver(provider, None)
                .execute(&f.reference, &references(), &f.action, &actor())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .unwrap();
    let consumed = faults.consumed();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    provider.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() == consumed {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        f.state
            .get(&key(
                &f,
                RESULT_KIND,
                attempt(f.reference.execution_id(), 0)
            ))
            .await
            .unwrap()
            .is_some()
    );
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    let receipt = f
        .history(None)
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        receipt.receipt.status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert!(receipt.attempts[0].original_outcome.is_none());
    assert!(receipt.attempts[0].original_evidence.is_none());
    assert_eq!(
        before,
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
    );
    release.send(()).unwrap();
    completed(
        &tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .status,
    );
    completed(
        &f.history(None)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .receipt
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn history_reports_prepared_work_without_inventing_a_delivery_identity() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(faults.clone(), provider.clone(), false, config()).await;
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .is_err()
    );
    let receipt = f
        .history(None)
        .inspect(&f.reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        receipt.receipt.status,
        GovernedProviderStatus::Prepared
    ));
    assert_public_history_wire(&receipt);
    assert_eq!(receipt.operation_integrity, OperationIntegrity::Unstarted);
    assert!(receipt.metadata.is_none());
    assert!(receipt.binding.is_none());
    assert!(receipt.attempts.is_empty());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.reference.execution_id().to_string()]
            .spent_units,
        0
    );
}

#[tokio::test]
async fn history_refuses_missing_original_operation_and_digest_pinned_result() {
    let provider = Arc::new(Counting::new(Mode::Success));
    let f = fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    completed(
        &f.driver(provider.clone(), None)
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap()
            .status,
    );
    let archive = f.history(None);
    let operation_key = key(&f, OPERATION_KIND, f.reference.execution_id().to_string());
    let original = f.state.get(&operation_key).await.unwrap().unwrap();
    f.state.delete(&operation_key).await.unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Unavailable)
    ));
    f.state.set(&operation_key, &original, None).await.unwrap();
    let output_key = key(&f, RESULT_KIND, attempt(f.reference.execution_id(), 0));
    let evidence = f.state.get(&output_key).await.unwrap().unwrap();
    f.state.delete(&output_key).await.unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Unavailable)
    ));
    f.state
        .set(&output_key, &format!("{evidence} "), None)
        .await
        .unwrap();
    assert!(matches!(
        archive.inspect(&f.reference, &actor()).await,
        Err(GovernedProviderError::Conflict)
    ));
    f.state.set(&output_key, &evidence, None).await.unwrap();
    completed(
        &archive
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .receipt
            .status,
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

fn assert_public_history_wire(
    history: &acteon_executor::governed::history::HistoricalProviderReceipt,
) {
    let wire = serde_json::to_value(history).unwrap();
    let public: acteon_core::ProviderExecutionHistory =
        serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(public).unwrap(), wire);
}

async fn pause_original_ack(
    f: &Fixture,
    faults: &FaultStore,
    worker: GovernedProviderExecutor,
) -> (
    tokio::task::JoinHandle<
        Result<acteon_executor::governed::GovernedProviderReceipt, GovernedProviderError>,
    >,
    tokio::sync::oneshot::Sender<()>,
) {
    let initial = faults.consumed();
    let (reached, resume) = faults
        .pause_next_read(
            KeyKind::Custom(RESULT_KIND.into()),
            acteon_state::testing::faults::ReadOperation::Get,
            FaultTiming::Before,
        )
        .unwrap();
    let context = f.reference.clone();
    let action = f.action.clone();
    let running = tokio::spawn(async move {
        worker
            .execute(&context, &references(), &action, &actor())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), reached)
        .await
        .unwrap()
        .unwrap();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    resume.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() < initial + 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    (running, release)
}

async fn assert_coordinator_unchanged(
    f: &Fixture,
    expected: &acteon_governance::CoordinatorSnapshot,
) {
    assert_eq!(
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

async fn reconciliation_policy(
    f: &Fixture,
) -> acteon_governance::reconciliation::ReconciliationCeiling {
    let state = f.coordinator.snapshot().await.unwrap();
    acteon_governance::reconciliation::ReconciliationCeiling {
        actor: PrincipalIdentity::new("operator", PrincipalKind::Human).unwrap(),
        subjects: vec![actor().id().into()],
        resources: state.starts[&attempt(f.reference.execution_id(), 0)]
            .resources
            .iter()
            .cloned()
            .collect(),
        valid_from_ms: 0,
        deadline_ms: 10_000,
    }
}
fn reconciliation_authorization<'a>(
    policy: &'a acteon_governance::reconciliation::ReconciliationCeiling,
    stamp: &'a acteon_governance::AuthorityStamp,
    clock: &'a ManualClock,
) -> acteon_governance::reconciliation::ReconciliationAuthorization<'a> {
    acteon_governance::reconciliation::ReconciliationAuthorization {
        ceiling: policy,
        evaluated_authority: stamp,
        clock,
        guard: None,
    }
}
async fn management_proof(
    f: &Fixture,
    driver: &GovernedProviderExecutor,
    policy: &acteon_governance::reconciliation::ReconciliationCeiling,
    stamp: &acteon_governance::AuthorityStamp,
) -> Vec<u8> {
    let descriptor = driver
        .reconciliation_attempt_evaluated(
            &f.reference,
            &actor(),
            0,
            reconciliation_authorization(policy, stamp, &f.clock),
        )
        .await
        .unwrap();
    finality_proof(
        descriptor,
        acteon_executor::governed::reconciliation::ProviderFinality::NoEffect {
            reason: "qualified external source fenced the attempt".into(),
        },
    )
}

#[tokio::test]
async fn evaluated_reconciliation_staged_proof_cannot_escape_a_closure_race() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    let resume = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let owner = actor();
    let pending = driver.reconcile_evaluated(
        &f.reference,
        &owner,
        0,
        &proof,
        reconciliation_authorization(&policy, &stamp, &f.clock),
    );
    tokio::pin!(pending);
    let closure = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while faults.consumed() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        f.coordinator
            .change(
                "close-during-finality",
                AuthorityChange::CloseResource {
                    resource: policy.resources[0].clone(),
                },
                "police",
                "maintenance",
            )
            .await
            .unwrap();
        resume.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(pending, closure);
    assert!(matches!(result, Err(GovernedProviderError::Conflict)));
    let before = f.coordinator.snapshot().await.unwrap();
    let restarted = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    assert!(matches!(
        restarted
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
        GovernedProviderStatus::ReconciliationRequired { .. }
    ));
    assert!(
        driver
            .reconcile(&f.reference, &actor(), &proof)
            .await
            .is_err()
    );
    let after = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    let fresh = after.stamp();
    assert!(matches!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &fresh, &f.clock)
            )
            .await
            .unwrap()
            .status,
        GovernedProviderStatus::Completed { .. }
    ));
    let state = f.coordinator.snapshot().await.unwrap();
    let root = &state.roots[&f.reference.execution_id().to_string()];
    assert_eq!((root.spent_units, root.active_attempts), (1, 0));
    assert!(state.closed_resources.contains(&policy.resources[0]));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn evaluated_reconciliation_retains_unacknowledged_original_in_one_authorized_cas() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    let (running, release) =
        pause_original_ack(&f, &faults, f.driver(provider.clone(), None)).await;
    let id = attempt(f.reference.execution_id(), 0);
    let before = f.coordinator.snapshot().await.unwrap();
    assert!(before.starts[&id].evidence.is_none());
    let policy = reconciliation_policy(&f).await;
    let stamp = before.stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    assert_eq!(
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await
            .is_err()
    );
    let failed = f.coordinator.snapshot().await.unwrap();
    assert!(failed.starts[&id].evidence.is_none());
    assert!(failed.starts[&id].reconciliation.is_none());
    assert_eq!(
        failed.roots[&f.reference.execution_id().to_string()].active_attempts,
        1
    );
    driver
        .reconcile_evaluated(
            &f.reference,
            &actor(),
            0,
            &proof,
            reconciliation_authorization(&policy, &stamp, &f.clock),
        )
        .await
        .unwrap();
    let settled = f.coordinator.snapshot().await.unwrap();
    let start = &settled.starts[&id];
    assert!(start.evidence.is_some());
    assert_eq!(
        start.reconciliation.as_ref().unwrap().original_evidence,
        start.evidence
    );
    assert_eq!(
        start.reconciliation.as_ref().unwrap().prior_status,
        acteon_governance::AttemptStatus::InFlight
    );
    assert_eq!(
        settled.roots[&f.reference.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(
        settled.roots[&f.reference.execution_id().to_string()].active_attempts,
        0
    );
    release.send(()).unwrap();
    assert!(matches!(
        running.await.unwrap().unwrap().status,
        GovernedProviderStatus::Completed { .. }
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn evaluated_reconciliation_lost_settlement_ack_cannot_bypass_operator_revocation() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await
            .is_err()
    );
    driver
        .reconcile_evaluated(
            &f.reference,
            &actor(),
            0,
            &proof,
            reconciliation_authorization(&policy, &stamp, &f.clock),
        )
        .await
        .unwrap();
    f.coordinator
        .change(
            "revoke-finality-operator",
            AuthorityChange::RevokeSubject {
                subject: "operator".into(),
            },
            "admin",
            "offboarding",
        )
        .await
        .unwrap();
    let fresh = f.coordinator.snapshot().await.unwrap().stamp();
    assert!(matches!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &fresh, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Admission(_))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn evaluated_reconciliation_cannot_replace_a_known_result_awaiting_ack() {
    use acteon_executor::governed::reconciliation::RECONCILIATION_KIND;
    for mode in [Mode::Success, Mode::RejectFirst] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let provider = Arc::new(Counting::new(mode));
        let mut settings = config();
        settings.max_retries = 0;
        let f = management_fixture(faults.clone(), provider.clone(), true, settings).await;
        let driver = f
            .driver(provider.clone(), None)
            .with_trusted_reconciliation_verifier(finality_verifier())
            .unwrap();
        let (running, release) =
            pause_original_ack(&f, &faults, f.driver(provider.clone(), None)).await;
        let before = f.coordinator.snapshot().await.unwrap();
        let policy = reconciliation_policy(&f).await;
        let stamp = before.stamp();
        let proof = management_proof(&f, &driver, &policy, &stamp).await;
        assert!(matches!(
            driver
                .reconcile_evaluated(
                    &f.reference,
                    &actor(),
                    0,
                    &proof,
                    reconciliation_authorization(&policy, &stamp, &f.clock)
                )
                .await,
            Err(GovernedProviderError::Conflict)
        ));
        let key = StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(RECONCILIATION_KIND.into()),
            attempt(f.reference.execution_id(), 0),
        );
        assert!(f.state.get(&key).await.unwrap().is_none());
        assert_eq!(
            serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        release.send(()).unwrap();
        running.await.unwrap().unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn evaluated_reconciliation_refuses_bad_proof_and_partial_bounds_without_staging() {
    use acteon_executor::governed::reconciliation::RECONCILIATION_KIND;
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let before = f.coordinator.snapshot().await.unwrap();
    let mut policy = reconciliation_policy(&f).await;
    let stamp = before.stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    assert!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                b"invalid-proof",
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await
            .is_err()
    );
    policy.resources.pop();
    assert!(matches!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Admission(_))
    ));
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(RECONCILIATION_KIND.into()),
        attempt(f.reference.execution_id(), 0),
    );
    assert!(f.state.get(&key).await.unwrap().is_none());
    assert_coordinator_unchanged(&f, &before).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn evaluated_reconciliation_lost_proof_ack_requires_fresh_authorized_acceptance() {
    use acteon_executor::governed::reconciliation::RECONCILIATION_KIND;
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
    let driver = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let mut policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    faults
        .fail_next(
            KeyKind::Custom(RECONCILIATION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await
            .is_err()
    );
    let restarted = f
        .driver(provider.clone(), None)
        .with_trusted_reconciliation_verifier(finality_verifier())
        .unwrap();
    let staged = f.coordinator.snapshot().await.unwrap();
    assert!(matches!(
        restarted
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .status,
        GovernedProviderStatus::ReconciliationRequired { .. }
    ));
    assert_coordinator_unchanged(&f, &staged).await;
    f.coordinator
        .change(
            "offboard-before-proof-acceptance",
            AuthorityChange::RevokeSubject {
                subject: "operator".into(),
            },
            "admin",
            "offboarding",
        )
        .await
        .unwrap();
    let fresh = f.coordinator.snapshot().await.unwrap().stamp();
    assert!(matches!(
        restarted
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &fresh, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Admission(_))
    ));
    let still_pending = f.coordinator.snapshot().await.unwrap();
    assert!(
        still_pending.starts[&attempt(f.reference.execution_id(), 0)]
            .reconciliation
            .is_none()
    );
    assert_eq!(
        still_pending.roots[&f.reference.execution_id().to_string()].active_attempts,
        1
    );
    policy.actor = PrincipalIdentity::new("replacement-operator", PrincipalKind::Human).unwrap();
    restarted
        .reconcile_evaluated(
            &f.reference,
            &actor(),
            0,
            &proof,
            reconciliation_authorization(&policy, &fresh, &f.clock),
        )
        .await
        .unwrap();
    let settled = f.coordinator.snapshot().await.unwrap();
    let root = &settled.roots[&f.reference.execution_id().to_string()];
    assert_eq!((root.spent_units, root.active_attempts), (1, 0));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn evaluated_reconciliation_history_retains_acceptance_after_replay_and_restart() {
    for encrypted in [false, true] {
        let provider = Arc::new(Counting::new(Mode::Connection));
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
        let encryptor = encrypted.then(|| {
            Arc::new(PayloadEncryptor::new(
                parse_master_key(&"02".repeat(32)).unwrap(),
            ))
        });
        let driver = f
            .driver(provider.clone(), encryptor.clone())
            .with_trusted_reconciliation_verifier(finality_verifier())
            .unwrap();
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap();
        let mut policy = reconciliation_policy(&f).await;
        let stamp = f.coordinator.snapshot().await.unwrap().stamp();
        let proof = management_proof(&f, &driver, &policy, &stamp).await;
        faults
            .fail_next(
                KeyKind::Custom(
                    acteon_executor::governed::reconciliation::RECONCILIATION_KIND.into(),
                ),
                WriteOperation::CheckAndSet,
                FaultTiming::After,
            )
            .unwrap();
        assert!(
            driver
                .reconcile_evaluated(
                    &f.reference,
                    &actor(),
                    0,
                    &proof,
                    reconciliation_authorization(&policy, &stamp, &f.clock)
                )
                .await
                .is_err()
        );
        f.clock.advance_to(Duration::from_millis(100)).unwrap();
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock),
            )
            .await
            .unwrap();
        f.clock.advance_to(Duration::from_millis(300)).unwrap();
        policy.actor = PrincipalIdentity::new("replacement", PrincipalKind::Human).unwrap();
        driver
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock),
            )
            .await
            .unwrap();
        let history = f
            .history(encryptor.clone())
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap();
        let resolution = history.attempts[0].reconciliation.as_ref().unwrap();
        let acceptance = resolution.acceptance.as_ref().unwrap();
        assert_eq!(acceptance.operator.id(), "operator");
        assert_eq!(acceptance.authority, stamp);
        assert_eq!(acceptance.accepted_at_ms, 200);
        assert_eq!(resolution.resolved_at_ms, 100);
        let wire: acteon_core::ProviderExecutionHistory =
            serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        assert_eq!(
            wire.attempts[0]
                .reconciliation
                .as_ref()
                .unwrap()
                .acceptance
                .as_ref()
                .unwrap()
                .operator
                .id(),
            "operator"
        );
        let restarted = f.driver(provider.clone(), encryptor);
        let retained = restarted
            .reconciliation_record(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.acceptance, resolution.acceptance);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

fn retired_reconciliation_store(
    f: &Fixture,
    digest: String,
    verifier: Arc<dyn acteon_executor::governed::reconciliation::ProviderReconciliationVerifier>,
    encryptor: Option<Arc<PayloadEncryptor>>,
) -> acteon_executor::governed::reconciliation::ProviderReconciliationStore {
    acteon_executor::governed::reconciliation::ProviderReconciliationStore::new_trusted(
        f.state.clone(),
        f.coordinator.clone(),
        f.contexts.clone(),
        f.clock.clone(),
        encryptor,
        std::collections::BTreeMap::from([(digest, verifier)]),
    )
    .unwrap()
}

#[tokio::test]
async fn retired_reconciliation_accepts_without_retaining_a_provider_and_survives_key_rotation() {
    for encrypted in [false, true] {
        let provider = Arc::new(Counting::new(Mode::Connection));
        let weak = Arc::downgrade(&provider);
        let f = management_fixture(
            Arc::new(MemoryStateStore::new()),
            provider.clone(),
            false,
            config(),
        )
        .await;
        let encryptor = encrypted.then(|| {
            Arc::new(PayloadEncryptor::new(
                parse_master_key(&"42".repeat(32)).unwrap(),
            ))
        });
        let digest = bound(provider.clone(), false)
            .reconciliation_binding_digest()
            .unwrap();
        let driver = f.driver(provider.clone(), encryptor.clone());
        driver
            .execute(&f.reference, &references(), &f.action, &actor())
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        drop(driver);
        drop(provider);
        assert!(
            weak.upgrade().is_none(),
            "archive must not keep any provider alive"
        );
        let archive = retired_reconciliation_store(
            &f,
            digest.clone(),
            finality_verifier(),
            encryptor.clone(),
        );
        let policy = reconciliation_policy(&f).await;
        let stamp = f.coordinator.snapshot().await.unwrap().stamp();
        let attempt = archive
            .reconciliation_attempt_evaluated(
                &f.reference,
                &actor(),
                0,
                reconciliation_authorization(&policy, &stamp, &f.clock),
            )
            .await
            .unwrap();
        assert_eq!(attempt.binding_digest, digest);
        let proof = finality_proof(
            attempt,
            acteon_executor::governed::reconciliation::ProviderFinality::NoEffect {
                reason: "qualified retired source fenced every possible delivery".into(),
            },
        );
        archive
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock),
            )
            .await
            .unwrap();
        let original = f.coordinator.snapshot().await.unwrap();
        drop(archive);
        let rotated = Arc::new(
            acteon_executor::governed::reconciliation::HmacFinalityVerifier::new_trusted(
                "provider-finality-v2",
                std::collections::BTreeMap::from([("replacement".into(), vec![83; 32])]),
            )
            .unwrap(),
        );
        let archive = retired_reconciliation_store(&f, digest, rotated, encryptor.clone());
        archive
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock),
            )
            .await
            .unwrap();
        assert_coordinator_unchanged(&f, &original).await;
        let retained = f
            .history(encryptor)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap();
        assert!(
            retained.attempts[0]
                .reconciliation
                .as_ref()
                .unwrap()
                .acceptance
                .is_some()
        );
    }
}

#[tokio::test]
async fn retired_reconciliation_rejects_wrong_binding_owner_and_unaccepted_old_key() {
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f.driver(provider.clone(), None);
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let digest = bound(provider.clone(), false)
        .reconciliation_binding_digest()
        .unwrap();
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let before = f.coordinator.snapshot().await.unwrap();
    let wrong = retired_reconciliation_store(&f, "00".repeat(32), finality_verifier(), None);
    assert!(matches!(
        wrong
            .reconciliation_attempt_evaluated(
                &f.reference,
                &actor(),
                0,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Conflict)
    ));
    let archive = retired_reconciliation_store(&f, digest.clone(), finality_verifier(), None);
    let other = PrincipalIdentity::new("other", PrincipalKind::Human).unwrap();
    assert!(matches!(
        archive
            .reconciliation_attempt_evaluated(
                &f.reference,
                &other,
                0,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Ownership)
    ));
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    let rotated = Arc::new(
        acteon_executor::governed::reconciliation::HmacFinalityVerifier::new_trusted(
            "provider-finality-v2",
            std::collections::BTreeMap::from([("replacement".into(), vec![83; 32])]),
        )
        .unwrap(),
    );
    let archive = retired_reconciliation_store(&f, digest, rotated, None);
    assert!(matches!(
        archive
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Conflict)
    ));
    assert_coordinator_unchanged(&f, &before).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retired_reconciliation_staged_receipt_requires_retained_verifier_revision() {
    use acteon_executor::governed::reconciliation::{
        HmacFinalityVerifier, ProviderFinality, sign_finality_receipt,
    };
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(faults.clone(), provider.clone(), false, config()).await;
    f.driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let digest = bound(provider.clone(), false)
        .reconciliation_binding_digest()
        .unwrap();
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let before = f.coordinator.snapshot().await.unwrap();
    let archive = retired_reconciliation_store(&f, digest.clone(), finality_verifier(), None);
    let attempt = archive
        .reconciliation_attempt_evaluated(
            &f.reference,
            &actor(),
            0,
            reconciliation_authorization(&policy, &stamp, &f.clock),
        )
        .await
        .unwrap();
    let finality = ProviderFinality::NoEffect {
        reason: "source fenced delivery".into(),
    };
    let proof = finality_proof(attempt.clone(), finality.clone());
    faults
        .fail_next(
            KeyKind::Custom(acteon_executor::governed::reconciliation::RECONCILIATION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        archive
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &proof,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Unavailable)
    ));
    assert_coordinator_unchanged(&f, &before).await;
    drop(archive);
    let rotated = Arc::new(
        HmacFinalityVerifier::new_trusted(
            "provider-finality-v2",
            std::collections::BTreeMap::from([("replacement".into(), vec![83; 32])]),
        )
        .unwrap(),
    );
    let archive = retired_reconciliation_store(&f, digest.clone(), rotated, None);
    let replacement = sign_finality_receipt(
        attempt,
        finality,
        "provider-finality-v2",
        "replacement",
        &[83; 32],
    )
    .unwrap();
    assert!(matches!(
        archive
            .reconcile_evaluated(
                &f.reference,
                &actor(),
                0,
                &replacement,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Conflict)
    ));
    assert_coordinator_unchanged(&f, &before).await;
    drop(archive);
    retired_reconciliation_store(&f, digest, finality_verifier(), None)
        .reconcile_evaluated(
            &f.reference,
            &actor(),
            0,
            &proof,
            reconciliation_authorization(&policy, &stamp, &f.clock),
        )
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retired_reconciliation_refuses_unsealed_legacy_work() {
    let state = Arc::new(MemoryStateStore::new());
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(state.clone(), provider.clone(), false, config()).await;
    f.driver(provider.clone(), None)
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let raw = state.get(&key).await.unwrap().unwrap();
    let mut legacy: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for start in legacy["starts"].as_object_mut().unwrap().values_mut() {
        start["operation_evidence"] = serde_json::Value::Null;
    }
    state
        .set(&key, &serde_json::to_string(&legacy).unwrap(), None)
        .await
        .unwrap();
    assert_eq!(
        f.history(None)
            .inspect(&f.reference, &actor())
            .await
            .unwrap()
            .unwrap()
            .operation_integrity,
        OperationIntegrity::Legacy
    );
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let archive = retired_reconciliation_store(
        &f,
        bound(provider.clone(), false)
            .reconciliation_binding_digest()
            .unwrap(),
        finality_verifier(),
        None,
    );
    let before = f.coordinator.snapshot().await.unwrap();
    assert!(matches!(
        archive
            .reconciliation_attempt_evaluated(
                &f.reference,
                &actor(),
                0,
                reconciliation_authorization(&policy, &stamp, &f.clock)
            )
            .await,
        Err(GovernedProviderError::Conflict)
    ));
    assert_coordinator_unchanged(&f, &before).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

struct ReconciliationFreshness(std::sync::atomic::AtomicBool);
#[async_trait]
impl acteon_governance::reconciliation::ReconciliationAuthorityGuard for ReconciliationFreshness {
    async fn check_current(&self) -> Result<(), acteon_governance::CoordinationError> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(acteon_governance::CoordinationError::Restricted)
        }
    }
}
struct SourceRevokingVerifier {
    freshness: Arc<ReconciliationFreshness>,
    qualified: Arc<dyn acteon_executor::governed::reconciliation::ProviderReconciliationVerifier>,
}
impl acteon_executor::governed::reconciliation::ProviderReconciliationVerifier
    for SourceRevokingVerifier
{
    fn revision(&self) -> &str {
        self.qualified.revision()
    }
    fn verify(
        &self,
        attempt: &acteon_executor::governed::reconciliation::ReconciliationAttempt,
        proof: &[u8],
    ) -> Result<acteon_executor::governed::reconciliation::ProviderFinality, GovernedProviderError>
    {
        let finality = self.qualified.verify(attempt, proof)?;
        self.freshness.0.store(false, Ordering::SeqCst);
        Ok(finality)
    }
}
#[tokio::test]
async fn reconciliation_host_freshness_loss_during_verification_prevents_proof_staging() {
    let provider = Arc::new(Counting::new(Mode::Connection));
    let f = management_fixture(
        Arc::new(MemoryStateStore::new()),
        provider.clone(),
        false,
        config(),
    )
    .await;
    let driver = f.driver(provider.clone(), None);
    driver
        .execute(&f.reference, &references(), &f.action, &actor())
        .await
        .unwrap();
    let policy = reconciliation_policy(&f).await;
    let stamp = f.coordinator.snapshot().await.unwrap().stamp();
    let proof = management_proof(&f, &driver, &policy, &stamp).await;
    let freshness = Arc::new(ReconciliationFreshness(true.into()));
    let verifier = Arc::new(SourceRevokingVerifier {
        freshness: freshness.clone(),
        qualified: finality_verifier(),
    });
    let archive = retired_reconciliation_store(
        &f,
        bound(provider.clone(), false)
            .reconciliation_binding_digest()
            .unwrap(),
        verifier,
        None,
    );
    let before = f.coordinator.snapshot().await.unwrap();
    let mut authorization = reconciliation_authorization(&policy, &stamp, &f.clock);
    authorization.guard = Some(freshness.as_ref());
    assert!(matches!(
        archive
            .reconcile_evaluated(&f.reference, &actor(), 0, &proof, authorization)
            .await,
        Err(GovernedProviderError::Admission(_))
    ));
    assert_coordinator_unchanged(&f, &before).await;
    assert_eq!(
        f.state
            .scan_keys_by_kind(KeyKind::Custom(
                acteon_executor::governed::reconciliation::RECONCILIATION_KIND.into(),
            ))
            .await
            .unwrap(),
        Vec::new(),
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}
