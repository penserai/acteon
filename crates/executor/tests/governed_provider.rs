use acteon_core::{
    Action, ActionOutcome, ExecutionContextReference, PrincipalIdentity, PrincipalKind,
    ProviderResponse, ResourceKind, ResourceRef,
};
use acteon_crypto::{PayloadEncryptor, parse_master_key};
use acteon_executor::governed::{
    BoundProvider, GovernedProviderError, GovernedProviderExecutor, GovernedProviderStatus,
    OPERATION_KIND, ProviderFailureContract, RESULT_KIND, governed_provider_input_digest,
};
use acteon_executor::{ExecutorConfig, RetryStrategy};
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
    let binding = BoundProvider::new_trusted(
        provider.clone(),
        &endpoint(),
        "work",
        "definition-v1",
        vec![],
    )
    .unwrap();
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
    catalog.resolve(&probe, &provider).unwrap().clone()
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
    use acteon_governance::context::CONTEXT_KIND;
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let settings = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("governed-provider-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let state: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
    let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
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
    provider.release.add_permits(1);
    completed(
        &tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .status,
    );
    completed(
        &replacement
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
    ] {
        assert!(peer.delete(&key(&f, kind, id)).await.unwrap());
    }
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
