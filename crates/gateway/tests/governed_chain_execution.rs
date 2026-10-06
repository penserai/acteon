//! Actual chain-engine calls under persisted, credentialed plan authority.
use acteon_core::{
    Action, ActionError, ActionOutcome, ChainConfig, ChainStepConfig, ParallelStepGroup,
    PrincipalIdentity, PrincipalKind, ProviderResponse, ResourceKind, ResourceRef,
};
use acteon_executor::{
    ExecutorConfig, GovernedProviderMediator, ProviderExecutionAdmission,
    ProviderExecutionAuthority, ProviderExecutionMediator, ProviderInvocation,
};
use acteon_executor::{
    catalog::QualifiedProviderCatalog,
    governed::{BoundProvider, GovernedProviderExecutor},
    plan::{QualifiedChainPlan, engine::StoredChainExecution, handoff::PlanHandoffStore},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinatorLimits, RootBudgetLimits, ScopePurpose,
    context::{
        ContextBinding, ContextSigningKey, ExecutionContextHandle, RootContextAdmission,
        TrustedContextStore,
    },
    credential::{CredentialAuthority, CredentialReference},
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference},
};
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
use acteon_time::ManualClock;
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Counter {
    calls: AtomicUsize,
    ambiguous: AtomicBool,
    blocking: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl DynProvider for Counter {
    fn name(&self) -> &str {
        "incident"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.blocking.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self.ambiguous.load(Ordering::SeqCst) && action.payload["source"] == "metrics" {
            return Err(ProviderError::Connection("completion unknown".into()));
        }
        Ok(ProviderResponse::success(action.payload.clone()))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("agent/maya", PrincipalKind::Agent).unwrap()
}
fn limits(units: u64) -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: units,
        max_concurrent: 2,
        deadline_ms: 1000,
    }
}
fn permits() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "work".into(),
        accepted_revision: 1,
    }]
}
fn definition() -> ChainConfig {
    ChainConfig::new("diagnose")
        .with_step(ChainStepConfig::new(
            "metrics",
            "incident",
            "execute",
            json!({"source":"metrics"}),
        ))
        .with_step(ChainStepConfig::new(
            "logs",
            "incident",
            "execute",
            json!({"source":"logs"}),
        ))
}
struct HostAdmission {
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
    handoffs: Arc<PlanHandoffStore>,
    catalog: QualifiedProviderCatalog,
    clock: Arc<ManualClock>,
}
fn refused() -> ActionError {
    ActionError {
        code: "TEST_HOST_REFUSED".into(),
        message: "host authority refused".into(),
        retryable: false,
        attempts: 0,
    }
}
#[async_trait]
impl ProviderExecutionAdmission for HostAdmission {
    async fn admit(
        &self,
        _: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        Err(refused())
    }
    async fn admit_chain(
        &self,
        job_id: uuid::Uuid,
        action: &Action,
        entry: &str,
        definitions: &BTreeMap<String, ChainConfig>,
    ) -> Result<(), ActionError> {
        let plan = Arc::new(
            QualifiedChainPlan::new_trusted(
                "city",
                "tenant",
                entry,
                definitions,
                self.catalog.clone(),
            )
            .map_err(|_| refused())?,
        );
        let bound = plan.bind_input(action).map_err(|_| refused())?;
        let key = format!("chain-job/{job_id}");
        let root = self
            .contexts
            .capture_idempotent_credentialed_root(
                &key,
                RootContextAdmission {
                    handle: ExecutionContextHandle::new(),
                    binding: ContextBinding {
                        execution_id: uuid::Uuid::new_v4(),
                        principal: actor(),
                        request_digest: bound.request_digest().into(),
                    },
                    credential_id: "private-key".into(),
                    auth_method: "api_key".into(),
                    accepted_ceiling_revision: String::new(),
                    accepted_effects: plan.required_effects().to_vec(),
                    deadline_ms: 1000,
                    evaluated_authority: self
                        .coordinator
                        .snapshot()
                        .await
                        .map_err(|_| refused())?
                        .stamp(),
                },
                &permits(),
                CredentialReference {
                    id: "private-key".into(),
                    accepted_revision: 1,
                },
                limits(2),
                self.clock.as_ref(),
            )
            .await
            .map_err(|_| refused())?;
        self.handoffs
            .persist(job_id, &bound, action, &root, &permits())
            .await
            .map_err(|_| refused())
    }
}
struct Fixture {
    coordinator: AuthorityCoordinator,
    state: Arc<FaultStore>,
    counter: Arc<Counter>,
    host: HostAdmission,
    providers: Arc<dyn ProviderExecutionMediator>,
    chain: Arc<StoredChainExecution>,
}
impl Fixture {
    async fn new(config: ChainConfig) -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp_millis(100).unwrap(),
        ));
        let state = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::with_clock(
            clock.clone(),
        ))));
        let coordinator = AuthorityCoordinator::initialize(
            state.clone(),
            "city",
            "tenant",
            CoordinatorLimits::default(),
        )
        .await
        .unwrap();
        coordinator
            .reserve_scope(ScopePurpose::Execution)
            .await
            .unwrap();
        let counter = Arc::new(Counter {
            calls: AtomicUsize::new(0),
            ambiguous: AtomicBool::new(false),
            blocking: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let selected: Arc<dyn DynProvider> = counter.clone();
        let binding = BoundProvider::new_trusted(
            selected,
            &ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "incident").unwrap(),
            "execute",
            "v1",
            vec![],
        )
        .unwrap()
        .qualify_for_catalog()
        .unwrap();
        let catalog = QualifiedProviderCatalog::new_trusted(vec![binding.clone()]).unwrap();
        let plan = QualifiedChainPlan::new_trusted(
            "city",
            "tenant",
            "diagnose",
            &BTreeMap::from([("diagnose".into(), config)]),
            catalog.clone(),
        )
        .unwrap();
        let ceiling = PermitIssuanceCeiling {
            issuer: actor(),
            subjects: vec![actor()],
            effects: plan.required_effects().to_vec(),
            valid_from_ms: 0,
            limits: limits(10),
        };
        let permit = ExecutionPermit {
            id: "work".into(),
            revision: 1,
            subject: actor(),
            effects: plan.required_effects().to_vec(),
            valid_from_ms: 0,
            limits: limits(10),
        };
        coordinator
            .publish_permit(
                "issue",
                permit.clone(),
                0,
                &ceiling,
                &coordinator.snapshot().await.unwrap().stamp(),
                "qualified host",
                100,
            )
            .await
            .unwrap();
        coordinator
            .publish_credential(
                "credential",
                CredentialAuthority {
                    ceiling: ExecutionPermit {
                        id: "private-key".into(),
                        ..permit
                    },
                    auth_method: "api_key".into(),
                    execution_enabled: true,
                },
                0,
                &ceiling,
                &coordinator.snapshot().await.unwrap().stamp(),
                "independent credential",
                100,
            )
            .await
            .unwrap();
        let contexts = Arc::new(
            TrustedContextStore::new(
                state.clone(),
                coordinator.clone(),
                "chain-engine".into(),
                "key".into(),
                vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
            )
            .unwrap(),
        );
        let handoffs = Arc::new(PlanHandoffStore::new(state.clone(), "city", "tenant").unwrap());
        let driver = GovernedProviderExecutor::new(
            state.clone(),
            coordinator.clone(),
            contexts.clone(),
            binding,
            ExecutorConfig::default(),
            clock.clone(),
            None,
        )
        .unwrap();
        let providers: Arc<dyn ProviderExecutionMediator> =
            Arc::new(GovernedProviderMediator::new(vec![driver]).unwrap());
        let chain = Arc::new(
            StoredChainExecution::new_trusted(
                handoffs.clone(),
                contexts.clone(),
                coordinator.clone(),
                catalog.clone(),
                providers.clone(),
                clock.clone(),
            )
            .unwrap(),
        );
        let host = HostAdmission {
            coordinator: coordinator.clone(),
            contexts,
            handoffs,
            catalog,
            clock,
        };
        Self {
            coordinator,
            state,
            counter,
            host,
            providers,
            chain,
        }
    }
    fn gateway(&self, config: ChainConfig) -> Gateway {
        let mut gateway = GatewayBuilder::new()
            .state(self.state.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(
                self.host.clock.clone(),
            )))
            .clock(self.host.clock.clone())
            .provider(self.counter.clone())
            .provider_execution_mediator(self.providers.clone())
            .chain(config)
            .rules(vec![Rule::new(
                "diagnose",
                Expr::Bool(true),
                RuleAction::Chain {
                    chain: "diagnose".into(),
                },
            )])
            .build()
            .unwrap();
        gateway.install_chain_execution_mediator(self.chain.clone());
        gateway
    }
    async fn start(&self, gateway: &Gateway) -> String {
        let outcome = gateway
            .dispatch_with_execution_admission(
                Action::new(
                    "city",
                    "tenant",
                    "incident",
                    "execute",
                    json!({"incident":42}),
                ),
                None,
                &self.host,
            )
            .await
            .unwrap();
        let ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
            panic!("chain not started")
        };
        chain_id
    }
}

#[tokio::test]
async fn actual_engine_recovers_completed_provider_after_lost_step_projection_and_registry_change()
{
    let f = Fixture::new(definition()).await;
    let first = f.gateway(definition());
    let id = f.start(&first).await;
    f.state
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(first.advance_chain("city", "tenant", &id).await.is_err());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let mut changed = definition();
    changed.steps[0].payload_template = json!({"source":"changed"});
    let replacement = f.gateway(changed);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    let chain = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        chain.step_results[0]
            .as_ref()
            .unwrap()
            .response_body
            .as_ref()
            .unwrap()["source"],
        "metrics"
    );
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let roots: Vec<_> = snapshot
        .roots
        .values()
        .filter(|root| root.spent_units == 2)
        .collect();
    assert_eq!(roots.len(), 1);
    assert!(
        snapshot
            .roots
            .values()
            .all(|root| root.active_attempts == 0)
    );
}

#[tokio::test]
async fn closure_between_actual_engine_steps_blocks_next_provider() {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();

    f.coordinator
        .change(
            "close",
            AuthorityChange::CloseResource {
                resource: ResourceRef::new(ResourceKind::Chain, "city", "tenant", "diagnose")
                    .unwrap(),
            },
            "operator",
            "incident control",
        )
        .await
        .unwrap();
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn altered_owned_chain_input_cannot_borrow_pinned_authority() {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    let key = StateKey::new("city", "tenant", KeyKind::Chain, &id);
    let raw = f.state.get(&key).await.unwrap().unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    state["origin_action"]["payload"] = json!({"incident": "forged"});
    f.state
        .set(&key, &serde_json::to_string(&state).unwrap(), None)
        .await
        .unwrap();
    assert!(gateway.advance_chain("city", "tenant", &id).await.is_err());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn missing_pinned_job_cannot_be_recreated_from_chain_labels() {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom("governed_plan_job".into()),
        &id,
    );
    assert!(f.state.delete(&key).await.unwrap());
    assert!(gateway.advance_chain("city", "tenant", &id).await.is_err());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn parallel_engine_calls_charge_shared_root_and_recover_without_reexecution() {
    let config = ChainConfig::new("diagnose").with_step(ChainStepConfig::new_parallel(
        "collect",
        ParallelStepGroup {
            steps: definition().steps,
            join: Default::default(),
            on_failure: Default::default(),
            timeout_seconds: None,
            max_concurrency: Some(2),
        },
    ));
    let f = Fixture::new(config.clone()).await;
    let gateway = f.gateway(config);
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .roots
            .values()
            .filter(|root| root.spent_units == 2)
            .count(),
        1
    );
    assert!(
        snapshot
            .roots
            .values()
            .all(|root| root.active_attempts == 0)
    );
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
}

async fn completed_step_repairs_after_authority_stops(expire: bool) {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    f.state
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(gateway.advance_chain("city", "tenant", &id).await.is_err());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let before = f.coordinator.snapshot().await.unwrap();
    if expire {
        f.host
            .clock
            .advance_to(std::time::Duration::from_secs(2))
            .unwrap();
    } else {
        f.coordinator
            .change(
                "revoke",
                AuthorityChange::RevokeCredential {
                    credential_id: "private-key".into(),
                    expected_revision: 1,
                },
                "operator",
                "stop new effects",
            )
            .await
            .unwrap();
    }
    // A replacement worker observes the old result but cannot start the next step.
    let replacement = f.gateway(definition());
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    let state = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert!(state.step_results[0].as_ref().unwrap().success);
    assert_eq!(
        state.step_results[0]
            .as_ref()
            .unwrap()
            .response_body
            .as_ref()
            .unwrap()["source"],
        "metrics"
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let repaired = f.coordinator.snapshot().await.unwrap();
    assert_eq!(repaired.starts.len(), before.starts.len());
    assert_eq!(repaired.roots.len(), before.roots.len());
    assert_eq!(repaired.budget_parents, before.budget_parents);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    assert!(
        replacement
            .get_chain_status("city", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .step_results[1]
            .as_ref()
            .is_some_and(|result| !result.success)
    );
}
#[tokio::test]
async fn completed_step_repairs_after_credential_revocation_without_new_effect_authority() {
    completed_step_repairs_after_authority_stops(false).await;
}
#[tokio::test]
async fn completed_step_repairs_after_expiry_without_new_effect_authority() {
    completed_step_repairs_after_authority_stops(true).await;
}

#[tokio::test]
async fn corrupt_retained_call_refuses_observation_without_execution_fallback() {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    f.state
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(gateway.advance_chain("city", "tenant", &id).await.is_err());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let records = f
        .state
        .scan_keys_by_kind(KeyKind::Custom("governed_plan_call".into()))
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let mut record: serde_json::Value = serde_json::from_str(&records[0].1).unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom("governed_plan_call".into()),
        record["admission_key"].as_str().unwrap(),
    );
    // A failed projection leaves its lock lease until expiry. Advance the
    // injected clock so the same worker can retry without waiting in real time.
    f.host
        .clock
        .advance_to(std::time::Duration::from_secs(65))
        .unwrap();
    record["input_digest"] = json!("a".repeat(64));
    f.state
        .set(&key, &serde_json::to_string(&record).unwrap(), None)
        .await
        .unwrap();
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    assert!(
        !gateway
            .get_chain_status("city", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .step_results[0]
            .as_ref()
            .unwrap()
            .success
    );
    assert_eq!(f.coordinator.snapshot().await.unwrap().starts.len(), 1);
}

#[tokio::test]
async fn uncertain_provider_step_parks_across_restart_and_expiry_without_resend() {
    let f = Fixture::new(definition()).await;
    f.counter.ambiguous.store(true, Ordering::SeqCst);
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    let before = f.coordinator.snapshot().await.unwrap();
    let parked = gateway
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.status, acteon_core::ChainStatus::WaitingProvider);
    assert_eq!(parked.step_attempts[0], 1);
    assert!(parked.step_results[0].is_none());
    let wait = parked.wait_state.clone().unwrap();
    let replacement = f.gateway(definition());
    f.host
        .clock
        .advance_to(std::time::Duration::from_secs(301))
        .unwrap();
    for _ in 0..3 {
        replacement
            .advance_chain("city", "tenant", &id)
            .await
            .unwrap();
        let state = replacement
            .get_chain_status("city", "tenant", &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.status, acteon_core::ChainStatus::WaitingProvider);
        assert_eq!(state.step_attempts[0], 1);
        assert!(state.step_results[0].is_none());
        let (
            acteon_core::WaitState::Provider {
                pending: original, ..
            },
            acteon_core::WaitState::Provider {
                pending: current, ..
            },
        ) = (&wait, state.wait_state.as_ref().unwrap())
        else {
            panic!("missing provider wait")
        };
        assert_eq!(original, current);
    }
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let after = f.coordinator.snapshot().await.unwrap();
    assert_eq!(after.starts.len(), before.starts.len());
    assert_eq!(after.roots, before.roots);
}

#[tokio::test]
async fn inflight_receipt_parks_then_recovers_completion_under_original_attempt() {
    let f = Fixture::new(definition()).await;
    f.counter.blocking.store(true, Ordering::SeqCst);
    let original = Arc::new(f.gateway(definition()));
    let id = f.start(&original).await;
    let running_id = id.clone();
    let running =
        tokio::spawn(async move { original.advance_chain("city", "tenant", &running_id).await });
    f.counter.entered.notified().await;
    let replacement = f.gateway(definition());
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    let parked = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parked.status, acteon_core::ChainStatus::WaitingProvider);
    assert_eq!(parked.step_attempts[0], 1);
    f.counter.blocking.store(false, Ordering::SeqCst);
    f.counter.release.notify_one();
    // The old worker loses its chain projection CAS, but its provider receipt survives.
    assert!(running.await.unwrap().is_err());
    f.coordinator
        .change(
            "revoke",
            AuthorityChange::RevokeCredential {
                credential_id: "private-key".into(),
                expected_revision: 1,
            },
            "operator",
            "stop effects",
        )
        .await
        .unwrap();
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    let repaired = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repaired.status, acteon_core::ChainStatus::Running);
    assert_eq!(repaired.step_attempts[0], 1);
    assert!(repaired.wait_state.is_none());
    assert!(repaired.step_results[0].as_ref().unwrap().success);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

fn parallel_definition(join: acteon_core::ParallelJoinPolicy, timeout: Option<u64>) -> ChainConfig {
    ChainConfig::new("diagnose").with_step(ChainStepConfig::new_parallel(
        "collect",
        ParallelStepGroup {
            steps: definition().steps,
            join,
            on_failure: Default::default(),
            timeout_seconds: timeout,
            max_concurrency: Some(2),
        },
    ))
}

#[tokio::test]
async fn parallel_pending_preserves_completed_sibling_and_does_not_restart_any_branch() {
    let config = parallel_definition(acteon_core::ParallelJoinPolicy::All, None);
    let f = Fixture::new(config.clone()).await;
    f.counter.ambiguous.store(true, Ordering::SeqCst);
    let gateway = f.gateway(config.clone());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    let state = gateway
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, acteon_core::ChainStatus::WaitingProvider);
    assert!(state.parallel_sub_results["logs"].success);
    assert!(!state.parallel_sub_results.contains_key("metrics"));
    let before = f.coordinator.snapshot().await.unwrap();
    let replacement = f.gateway(config);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    let state = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, acteon_core::ChainStatus::WaitingProvider);
    assert!(state.parallel_sub_results["logs"].success);
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots, before.roots);
}

#[tokio::test]
async fn parallel_any_preserves_uncertain_loser_and_never_starts_later_waves() {
    let mut config = parallel_definition(acteon_core::ParallelJoinPolicy::Any, None);
    config.steps[0]
        .parallel
        .as_mut()
        .unwrap()
        .steps
        .push(ChainStepConfig::new(
            "extra",
            "incident",
            "execute",
            json!({"source":"extra"}),
        ));
    let f = Fixture::new(config.clone()).await;
    f.counter.ambiguous.store(true, Ordering::SeqCst);
    let gateway = f.gateway(config.clone());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    let before = f.coordinator.snapshot().await.unwrap();
    let replacement = f.gateway(config);
    for _ in 0..2 {
        replacement
            .advance_chain("city", "tenant", &id)
            .await
            .unwrap();
    }
    let state = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, acteon_core::ChainStatus::WaitingProvider);
    assert!(state.parallel_sub_results["logs"].success);
    assert!(!state.parallel_sub_results.contains_key("extra"));
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots, before.roots);
}

#[tokio::test]
async fn parallel_timeout_parks_started_provider_receipts_without_resending() {
    let config = parallel_definition(acteon_core::ParallelJoinPolicy::All, Some(1));
    let f = Fixture::new(config.clone()).await;
    f.counter.blocking.store(true, Ordering::SeqCst);
    let gateway = Arc::new(f.gateway(config.clone()));
    let id = f.start(&gateway).await;
    let worker_id = id.clone();
    let running =
        tokio::spawn(async move { gateway.advance_chain("city", "tenant", &worker_id).await });
    f.counter.entered.notified().await;
    while f.counter.calls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    f.host
        .clock
        .advance_to(std::time::Duration::from_secs(1))
        .unwrap();
    running.await.unwrap().unwrap();
    let replacement = f.gateway(config);
    let state = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, acteon_core::ChainStatus::WaitingProvider);
    let Some(acteon_core::WaitState::Provider { pending, .. }) = state.wait_state else {
        panic!("missing retained work")
    };
    assert_eq!(pending.len(), 2);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        replacement
            .get_chain_status("city", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        acteon_core::ChainStatus::WaitingProvider
    );
}

#[tokio::test]
async fn cancellation_fences_already_admitted_child_and_preserves_other_instances() {
    use acteon_executor::plan::engine::{ChainExecutionMediator, ChainProviderCall};
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    let loaded = gateway
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    let job_id = uuid::Uuid::parse_str(&id).unwrap();
    let path = vec!["metrics".to_string()];
    let action = Action::new(
        "city",
        "tenant",
        "incident",
        "execute",
        json!({"source":"metrics"}),
    );
    let selected: Arc<dyn DynProvider> = f.counter.clone();
    // Simulate a stale worker that has already captured its signed child context.
    let authority = f
        .chain
        .admit(ChainProviderCall {
            namespace: "city",
            tenant: "tenant",
            job_id,
            chain_name: "diagnose",
            origin: &loaded.origin_action,
            step_path: &path,
            logical_attempt: uuid::Uuid::new_v4(),
            action: &action,
            selected: &selected,
        })
        .await
        .unwrap();
    gateway
        .cancel_chain(
            "city",
            "tenant",
            &id,
            Some("stop".into()),
            Some("operator".into()),
        )
        .await
        .unwrap();
    let outcome = f
        .providers
        .execute(ProviderInvocation {
            action: &action,
            selected: &selected,
            context: None,
            origin: acteon_executor::ProviderInvocationOrigin::ChainStep,
            authority: Some(&authority),
        })
        .await;
    assert!(matches!(outcome, ActionOutcome::Failed(_)));
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    // Another admitted instance of the same definition and principal remains usable.
    let other = f.start(&gateway).await;
    gateway
        .advance_chain("city", "tenant", &other)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .roots
            .values()
            .filter(|root| root.cancelled)
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_keeps_uncertain_receipt_and_charge_across_restart() {
    use acteon_executor::plan::engine::ChainExecutionMediator;
    let f = Fixture::new(definition()).await;
    f.counter.ambiguous.store(true, Ordering::SeqCst);
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    let before = gateway
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    let cancelled = gateway
        .cancel_chain("city", "tenant", &id, None, None)
        .await
        .unwrap();
    assert_eq!(cancelled.status, acteon_core::ChainStatus::Cancelled);
    assert_eq!(cancelled.wait_state, before.wait_state);
    // Fence retries are idempotent independently of workflow projection.
    let job_id = uuid::Uuid::parse_str(&id).unwrap();
    f.chain.cancel_job("city", "tenant", job_id).await.unwrap();
    let replacement = f.gateway(definition());
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let snapshot = f.coordinator.snapshot().await.unwrap();
    let root = snapshot.roots.values().find(|root| root.cancelled).unwrap();
    assert_eq!((root.spent_units, root.active_attempts), (1, 1));
}

#[tokio::test]
async fn cancellation_fence_survives_failed_chain_projection_and_retries() {
    let f = Fixture::new(definition()).await;
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    f.state
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        gateway
            .cancel_chain("city", "tenant", &id, None, None)
            .await
            .is_err()
    );
    assert_eq!(
        f.coordinator
            .snapshot()
            .await
            .unwrap()
            .roots
            .values()
            .filter(|root| root.cancelled)
            .count(),
        1
    );
    let replacement = f.gateway(definition());
    let cancelled = replacement
        .cancel_chain("city", "tenant", &id, None, None)
        .await
        .unwrap();
    assert_eq!(cancelled.status, acteon_core::ChainStatus::Cancelled);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.coordinator
            .snapshot()
            .await
            .unwrap()
            .changes
            .values()
            .filter(|record| matches!(record.change, AuthorityChange::CancelExecution { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_preserves_known_settlement_of_already_started_work() {
    let f = Fixture::new(definition()).await;
    f.counter.blocking.store(true, Ordering::SeqCst);
    let original = Arc::new(f.gateway(definition()));
    let id = f.start(&original).await;
    let running_id = id.clone();
    let running =
        tokio::spawn(async move { original.advance_chain("city", "tenant", &running_id).await });
    f.counter.entered.notified().await;
    let replacement = f.gateway(definition());
    replacement
        .cancel_chain("city", "tenant", &id, None, None)
        .await
        .unwrap();
    let before = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        before
            .roots
            .values()
            .find(|root| root.cancelled)
            .unwrap()
            .active_attempts,
        1
    );
    f.counter.blocking.store(false, Ordering::SeqCst);
    f.counter.release.notify_one();
    assert!(running.await.unwrap().is_err());
    let chain = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(chain.status, acteon_core::ChainStatus::Cancelled);
    let after = f.coordinator.snapshot().await.unwrap();
    let root = after.roots.values().find(|root| root.cancelled).unwrap();
    assert_eq!((root.spent_units, root.active_attempts), (1, 0));
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_survives_provider_catalog_removal_and_expired_authority() {
    let f = Fixture::new(definition()).await;
    let original = f.gateway(definition());
    let id = f.start(&original).await;
    f.host
        .clock
        .advance_to(std::time::Duration::from_secs(10))
        .unwrap();
    use acteon_executor::plan::engine::ChainExecutionMediator;
    let selected: Arc<dyn DynProvider> = f.counter.clone();
    let replacement_binding = BoundProvider::new_trusted(
        selected,
        &ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "incident").unwrap(),
        "other-operation",
        "v2",
        vec![],
    )
    .unwrap();
    let boundary = Arc::new(
        StoredChainExecution::new_trusted(
            f.host.handoffs.clone(),
            f.host.contexts.clone(),
            f.coordinator.clone(),
            QualifiedProviderCatalog::new_trusted(vec![replacement_binding]).unwrap(),
            f.providers.clone(),
            f.host.clock.clone(),
        )
        .unwrap(),
    );
    assert!(
        boundary
            .recover_job("city", "tenant", uuid::Uuid::parse_str(&id).unwrap())
            .await
            .is_err()
    );
    let mut replacement = f.gateway(definition());
    replacement.install_chain_execution_mediator(boundary);
    replacement
        .cancel_chain("city", "tenant", &id, None, None)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.coordinator
            .snapshot()
            .await
            .unwrap()
            .roots
            .values()
            .filter(|root| root.cancelled)
            .count(),
        1
    );
}

#[tokio::test]
async fn trusted_finality_resumes_original_parked_chain_step_without_a_second_send() {
    use acteon_executor::governed::reconciliation::{
        HmacFinalityVerifier, ProviderFinality, sign_finality_receipt,
    };
    let f = Fixture::new(definition()).await;
    f.counter.ambiguous.store(true, Ordering::SeqCst);
    let gateway = f.gateway(definition());
    let id = f.start(&gateway).await;
    gateway.advance_chain("city", "tenant", &id).await.unwrap();
    let parked = gateway
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    let Some(acteon_core::WaitState::Provider { pending, .. }) = &parked.wait_state else {
        panic!("provider should park")
    };
    let execution_id = pending[0].work.execution_id;
    let action = Action::new(
        "city",
        "tenant",
        "incident",
        "execute",
        json!({"source":"metrics"}),
    );
    let selected: Arc<dyn DynProvider> = f.counter.clone();
    let bound = f.host.catalog.resolve(&action, &selected).unwrap().clone();
    let driver = GovernedProviderExecutor::new(
        f.state.clone(),
        f.coordinator.clone(),
        f.host.contexts.clone(),
        bound,
        ExecutorConfig::default(),
        f.host.clock.clone(),
        None,
    )
    .unwrap()
    .with_trusted_reconciliation_verifier(Arc::new(
        HmacFinalityVerifier::new_trusted(
            "finality-v1",
            BTreeMap::from([("provider".into(), vec![53; 32])]),
        )
        .unwrap(),
    ))
    .unwrap();
    // The host resolves the signed context from durable operation provenance;
    // no public label or operator-chosen parent is admitted here.
    let operation_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_executor::governed::OPERATION_KIND.into()),
        execution_id.to_string(),
    );
    let operation: serde_json::Value =
        serde_json::from_str(&f.state.get(&operation_key).await.unwrap().unwrap()).unwrap();
    let reference: acteon_core::ExecutionContextReference =
        serde_json::from_value(operation["context"].clone()).unwrap();
    let attempt = driver
        .reconciliation_attempt(&reference, &actor())
        .await
        .unwrap()
        .unwrap();
    let proof = sign_finality_receipt(
        attempt,
        ProviderFinality::Completed {
            response: ProviderResponse::success(
                json!({"source":"metrics", "external_receipt":"committed"}),
            ),
        },
        "finality-v1",
        "provider",
        &[53; 32],
    )
    .unwrap();
    driver
        .reconcile(&reference, &actor(), &proof)
        .await
        .unwrap();
    let history = driver
        .reconciliation_record(&reference, &actor())
        .await
        .unwrap()
        .unwrap();
    assert!(history.original_evidence.is_some());
    assert_eq!(history.execution_id, execution_id);
    let replacement = f.gateway(definition());
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    let repaired = replacement
        .get_chain_status("city", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repaired.status, acteon_core::ChainStatus::Running);
    assert_eq!(repaired.step_attempts[0], 1);
    assert!(repaired.step_results[0].as_ref().unwrap().success);
    assert!(repaired.wait_state.is_none());
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    replacement
        .advance_chain("city", "tenant", &id)
        .await
        .unwrap();
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        replacement
            .get_chain_status("city", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        acteon_core::ChainStatus::Completed
    );
}
