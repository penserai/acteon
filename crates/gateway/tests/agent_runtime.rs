//! Actual governed provider calls behind durable agent-task acceptance.
use acteon_core::{
    Action, AgentCard, PrincipalIdentity, PrincipalKind, ProviderResponse, ResourceKind,
    ResourceRef, Skill, TaskMessage, TaskRole, TaskState, bus_agent_card::Interface,
};
use acteon_executor::{
    ExecutorConfig, RetryStrategy,
    delegation::{ApprovedPeerBinding, ApprovedServicePlan},
    governed::{BoundProvider, GovernedProviderStatus, governed_provider_input_digest},
};
use acteon_gateway::{
    TaskEngine, TaskScope,
    agent_runtime::{ACCEPTANCE_KIND, AgentProviderRuntime, AgentRuntimeDependencies},
};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinatorLimits, RootBudgetLimits,
    context::{
        AcceptedEffect, ContextBinding, ContextSigningKey, DelegatedContextAdmission,
        DelegatingRootAdmission, ExecutionContextHandle, RootContextAdmission, TrustedContextStore,
        VerifiedExecutionContext,
    },
    credential::{CredentialAuthority, CredentialReference},
    delegation_policy::{
        DelegationGrant, DelegationGrantReference, DelegationIssuanceCeiling,
        EvaluatedDelegationPublication,
    },
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference},
};
use acteon_provider::{DynProvider, ProviderError};
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
fn actor(id: &str) -> PrincipalIdentity {
    PrincipalIdentity::new(
        id,
        if id == "caller" || id == "operator" {
            PrincipalKind::Human
        } else {
            PrincipalKind::Agent
        },
    )
    .unwrap()
}
fn resource(kind: ResourceKind, id: &str) -> ResourceRef {
    ResourceRef::new(kind, "city", "tenant", id).unwrap()
}
fn limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 3,
        max_concurrent: 1,
        deadline_ms: 10_000,
    }
}
fn permits(id: &str) -> Vec<PermitReference> {
    vec![PermitReference {
        id: format!("{id}-permit"),
        accepted_revision: 1,
    }]
}
fn credential(id: &str) -> CredentialReference {
    CredentialReference {
        id: format!("{id}-key"),
        accepted_revision: 1,
    }
}
fn reference(id: &str) -> DelegationGrantReference {
    DelegationGrantReference {
        id: id.into(),
        accepted_revision: 1,
    }
}
fn issuer(grant: &DelegationGrant) -> DelegationIssuanceCeiling {
    DelegationIssuanceCeiling {
        issuer: actor("operator"),
        sources: vec![grant.source.clone()],
        targets: vec![grant.target.clone()],
        binding_digests: vec![grant.binding_digest.clone()],
        ingress_effects: vec![grant.ingress_effect.clone()],
        effects: grant.effects.clone(),
        valid_from_ms: 0,
        limits: limits(),
        max_depth: 3,
    }
}
async fn publish_actor(coordinator: &AuthorityCoordinator, id: &str, effects: Vec<AcceptedEffect>) {
    let ceiling = PermitIssuanceCeiling {
        issuer: actor("operator"),
        subjects: vec![actor(id)],
        effects: effects.clone(),
        valid_from_ms: 0,
        limits: limits(),
    };
    let permit = ExecutionPermit {
        id: format!("{id}-permit"),
        revision: 1,
        subject: actor(id),
        effects,
        valid_from_ms: 0,
        limits: limits(),
    };
    coordinator
        .publish_permit(
            &format!("{id}-issue"),
            permit.clone(),
            0,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "qualified",
            100,
        )
        .await
        .unwrap();
    coordinator
        .publish_credential(
            &format!("{id}-credential"),
            CredentialAuthority {
                ceiling: ExecutionPermit {
                    id: format!("{id}-key"),
                    ..permit
                },
                auth_method: "api_key".into(),
                execution_enabled: true,
            },
            0,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "authenticated",
            100,
        )
        .await
        .unwrap();
}
async fn publish_grant(
    coordinator: &AuthorityCoordinator,
    grant: DelegationGrant,
    clock: &ManualClock,
) {
    let ceiling = issuer(&grant);
    coordinator
        .publish_delegation_grant(EvaluatedDelegationPublication {
            change_id: &format!("{}-{}", grant.id, grant.revision),
            expected_revision: grant.revision - 1,
            grant,
            ceiling: &ceiling,
            evaluated_authority: &coordinator.snapshot().await.unwrap().stamp(),
            reason: "approved service",
            clock,
        })
        .await
        .unwrap();
}
async fn admission(
    coordinator: &AuthorityCoordinator,
    id: &str,
    effects: Vec<AcceptedEffect>,
) -> RootContextAdmission {
    RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor(id),
            request_digest: if id == "caller" { "a" } else { "b" }.repeat(64),
        },
        credential_id: format!("{id}-key"),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "host".into(),
        accepted_effects: effects,
        deadline_ms: 10_000,
        evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
    }
}

struct Counter {
    calls: AtomicUsize,
    ambiguous: bool,
    blocking: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[async_trait::async_trait]
impl DynProvider for Counter {
    fn name(&self) -> &'static str {
        "pager"
    }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.blocking.load(Ordering::SeqCst) {
            self.release.acquire().await.unwrap().forget();
        }
        if self.ambiguous {
            return Err(ProviderError::Connection("reply lost".into()));
        }
        Ok(ProviderResponse::success(action.payload.clone()))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}
struct Fixture {
    state: Arc<FaultStore>,
    coordinator: AuthorityCoordinator,
    contexts: Arc<TrustedContextStore>,
    clock: Arc<ManualClock>,
    counter: Arc<Counter>,
    bound: BoundProvider,
    card: AgentCard,
    parent: VerifiedExecutionContext,
    child: VerifiedExecutionContext,
    message: TaskMessage,
}
impl Fixture {
    fn binding(card: &AgentCard, bound: &BoundProvider) -> ApprovedPeerBinding {
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: std::iter::once(resource(ResourceKind::Agent, "notifier"))
                .chain(bound.effect().resources.iter().cloned())
                .collect(),
        };
        ApprovedPeerBinding::new_service_trusted(
            card,
            actor("worker"),
            "notify",
            "https://peer.example/a2a",
            "rest",
            ingress,
            ApprovedServicePlan::new_trusted(
                vec![bound.effect().clone()],
                vec![bound.effect().clone()],
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn create_runtime(
        state: Arc<dyn StateStore>,
        coordinator: AuthorityCoordinator,
        contexts: Arc<TrustedContextStore>,
        clock: Arc<ManualClock>,
        card: &AgentCard,
        bound: &BoundProvider,
    ) -> AgentProviderRuntime {
        AgentProviderRuntime::new_trusted(
            AgentRuntimeDependencies {
                state,
                coordinator,
                contexts,
                clock,
            },
            Self::binding(card, bound),
            bound.clone(),
            ExecutorConfig {
                max_retries: 0,
                max_concurrent: 1,
                execution_timeout: Duration::from_secs(1),
                retry_strategy: RetryStrategy::Constant {
                    delay: Duration::ZERO,
                },
            },
        )
        .unwrap()
    }
    fn runtime(&self) -> AgentProviderRuntime {
        Self::create_runtime(
            self.state.clone(),
            self.coordinator.clone(),
            self.contexts.clone(),
            self.clock.clone(),
            &self.card,
            &self.bound,
        )
    }
    async fn new(ambiguous: bool) -> Self {
        Self::new_with_state(
            Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new()))),
            ambiguous,
        )
        .await
    }
    // Keep the complete source/recipient authority fixture in one setup routine.
    #[allow(clippy::too_many_lines)]
    async fn new_with_state(state: Arc<FaultStore>, ambiguous: bool) -> Self {
        let coordinator = AuthorityCoordinator::initialize(
            state.clone(),
            "city",
            "tenant",
            CoordinatorLimits::default(),
        )
        .await
        .unwrap();
        coordinator
            .reserve_scope(acteon_governance::ScopePurpose::Execution)
            .await
            .unwrap();
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp_millis(100).unwrap(),
        ));
        let contexts = Arc::new(
            TrustedContextStore::new(
                state.clone(),
                coordinator.clone(),
                "city-domain".into(),
                "key".into(),
                vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
            )
            .unwrap(),
        );
        let counter = Arc::new(Counter {
            calls: AtomicUsize::new(0),
            ambiguous,
            blocking: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        let bound = BoundProvider::new_trusted(
            counter.clone(),
            &resource(ResourceKind::Endpoint, "pager-v1"),
            "notify",
            "reviewed-v1",
            vec![],
        )
        .unwrap()
        .qualify_for_catalog()
        .unwrap();
        let mut card = AgentCard::new("notifier", "city", "tenant", "Notifier", "v1");
        card.skills.push(Skill::new("notify"));
        card.interfaces.push(Interface {
            kind: "rest".into(),
            url: "https://peer.example/a2a".into(),
        });
        let binding = Self::binding(&card, &bound);
        let ingress = AcceptedEffect {
            operation: "agent.invoke".into(),
            resources: std::iter::once(resource(ResourceKind::Agent, "notifier"))
                .chain(bound.effect().resources.iter().cloned())
                .collect(),
        };
        publish_actor(&coordinator, "caller", vec![ingress.clone()]).await;
        publish_actor(&coordinator, "worker", vec![bound.effect().clone()]).await;
        let grant = DelegationGrant {
            id: "service".into(),
            revision: 1,
            source: actor("caller"),
            target: actor("worker"),
            agent_resource: resource(ResourceKind::Agent, "notifier"),
            binding_digest: binding.digest().into(),
            skill: "notify".into(),
            ingress_effect: ingress.clone(),
            effects: vec![bound.effect().clone()],
            valid_from_ms: 0,
            limits: limits(),
            max_depth: 3,
        };
        publish_grant(&coordinator, grant, &clock).await;
        let parent = contexts
            .capture_delegating_root(DelegatingRootAdmission {
                admission_key: "root",
                admission: admission(&coordinator, "caller", vec![ingress]).await,
                permits: &permits("caller"),
                credential: credential("caller"),
                grants: vec![reference("service")],
                limits: limits(),
                representation: None,
                clock: clock.as_ref(),
            })
            .await
            .unwrap();
        let message = TaskMessage::text("request-1", TaskRole::User, "Notify the operator");
        let runtime = Self::create_runtime(
            state.clone(),
            coordinator.clone(),
            contexts.clone(),
            clock.clone(),
            &card,
            &bound,
        );
        let action = runtime.prepare_message(&message).unwrap();
        let mut recipient = admission(&coordinator, "worker", vec![bound.effect().clone()]).await;
        recipient.binding.request_digest = governed_provider_input_digest(&action).unwrap();
        let child = contexts
            .capture_delegated_child(DelegatedContextAdmission {
                admission_key: "notify-input",
                parent: &parent,
                parent_permits: &permits("caller"),
                grant: reference("service"),
                binding_digest: binding.digest(),
                recipient,
                recipient_permits: &permits("worker"),
                credential: credential("worker"),
                representation: None,
                intent_effects: vec![bound.effect().clone()],
                onward_grants: vec![],
                limits: limits(),
                clock: clock.as_ref(),
            })
            .await
            .unwrap();
        Self {
            state,
            coordinator,
            contexts,
            clock,
            counter,
            bound,
            card,
            parent,
            child,
            message,
        }
    }
    async fn accept(&self, runtime: &AgentProviderRuntime) -> acteon_core::Task {
        runtime
            .accept(&self.child, &permits("worker"), &self.message)
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn accepted_task_executes_under_recipient_authority_and_shared_sponsorship() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    let task = f.accept(&runtime).await;
    assert_eq!(task.id, f.child.execution_id().to_string());
    assert_eq!(task.status.state, TaskState::Submitted);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    let result = Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(result.task.status.state, TaskState::Completed);
    assert_eq!(result.task.artifacts.len(), 1);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        state.roots[&f.parent.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(
        state.roots[&f.child.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(state.roots.len(), 2);
}
#[tokio::test]
async fn replacement_runtime_replays_original_task_and_result_without_a_new_call() {
    let f = Fixture::new(false).await;
    let first = f.runtime();
    f.accept(&first).await;
    Box::pin(first.resume(f.child.execution_id()))
        .await
        .unwrap();
    let replacement = f.runtime();
    let replay = f.accept(&replacement).await;
    assert_eq!(replay.status.state, TaskState::Completed);
    let result = Box::pin(replacement.resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(result.task.status.state, TaskState::Completed);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn lost_acceptance_or_task_acknowledgement_recovers_the_original_identity() {
    for kind in [KeyKind::Custom(ACCEPTANCE_KIND.into()), KeyKind::A2aTask] {
        let f = Fixture::new(false).await;
        let runtime = f.runtime();
        f.state
            .fail_next(kind, WriteOperation::CheckAndSet, FaultTiming::After)
            .unwrap();
        assert!(
            runtime
                .accept(&f.child, &permits("worker"), &f.message)
                .await
                .is_err()
        );
        let replay = f.accept(&f.runtime()).await;
        assert_eq!(replay.id, f.child.execution_id().to_string());
        assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
        let result = Box::pin(f.runtime().resume(f.child.execution_id()))
            .await
            .unwrap();
        assert_eq!(result.task.status.state, TaskState::Completed);
        assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn missing_task_projection_is_repaired_from_known_execution_evidence() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    f.accept(&runtime).await;
    Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap();
    f.state
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::A2aTask,
            f.child.execution_id().to_string(),
        ))
        .await
        .unwrap();
    let result = Box::pin(f.runtime().resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(result.task.status.state, TaskState::Completed);
    assert_eq!(result.task.artifacts.len(), 1);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn ambiguous_provider_result_remains_unresolved_across_restart_and_reaping() {
    let f = Fixture::new(true).await;
    let runtime = f.runtime();
    f.accept(&runtime).await;
    let result = Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(result.task.status.state, TaskState::Working);
    assert!(matches!(
        result.execution.unwrap().status,
        GovernedProviderStatus::ReconciliationRequired { .. }
    ));
    f.clock.advance_to(Duration::from_secs(120)).unwrap();
    let engine = TaskEngine::new(f.state.clone()).with_clock(f.clock.clone());
    assert!(
        engine
            .fail_if_stale(
                &TaskScope::new("city", "tenant"),
                &f.child.execution_id().to_string(),
                f.clock.now()
            )
            .await
            .unwrap()
            .is_none()
    );
    let recovered = Box::pin(f.runtime().resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(recovered.task.status.state, TaskState::Working);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        state.roots[&f.parent.execution_id().to_string()].active_attempts,
        1
    );
    assert_eq!(
        state.roots[&f.child.execution_id().to_string()].active_attempts,
        1
    );
}
#[tokio::test]
async fn cancellation_offboarding_or_closure_before_resume_refuses_the_provider_start() {
    for cause in ["cancel", "offboard", "close"] {
        let f = Fixture::new(false).await;
        let runtime = f.runtime();
        f.accept(&runtime).await;
        let change = match cause {
            "cancel" => AuthorityChange::CancelExecution {
                execution_id: f.parent.execution_id().to_string(),
            },
            "offboard" => AuthorityChange::RevokeSubject {
                subject: "caller".into(),
            },
            _ => AuthorityChange::CloseResource {
                resource: resource(ResourceKind::Agent, "notifier"),
            },
        };
        f.coordinator
            .change("intervene", change, "operator", cause)
            .await
            .unwrap();
        assert!(
            Box::pin(runtime.resume(f.child.execution_id()))
                .await
                .is_err()
        );
        assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
        assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
    }
}
#[tokio::test]
async fn substituted_input_or_original_agent_binding_cannot_be_accepted() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    let other = TaskMessage::text("request-1", TaskRole::User, "Different operation");
    assert!(
        runtime
            .accept(&f.child, &permits("worker"), &other)
            .await
            .is_err()
    );
    let mut card = f.card.clone();
    card.version = "v2".into();
    let substituted = Fixture::create_runtime(
        f.state.clone(),
        f.coordinator.clone(),
        f.contexts.clone(),
        f.clock.clone(),
        &card,
        &f.bound,
    );
    assert!(
        substituted
            .accept(&f.child, &permits("worker"), &f.message)
            .await
            .is_err()
    );
    assert!(
        f.state
            .get(&StateKey::new(
                "city",
                "tenant",
                KeyKind::Custom(ACCEPTANCE_KIND.into()),
                f.child.execution_id().to_string()
            ))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn concurrent_resumes_share_one_governed_execution() {
    let f = Fixture::new(false).await;
    let first = f.runtime();
    let second = f.runtime();
    f.accept(&first).await;
    let (a, b) = tokio::join!(
        Box::pin(first.resume(f.child.execution_id())),
        Box::pin(second.resume(f.child.execution_id()))
    );
    assert!(a.is_ok(), "first runtime failed");
    assert!(b.is_ok(), "second runtime failed");
    let final_result = Box::pin(f.runtime().resume(f.child.execution_id()))
        .await
        .unwrap();
    assert_eq!(final_result.task.status.state, TaskState::Completed);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_second_runtime_observes_the_actual_in_flight_attempt_without_calling_again() {
    let f = Fixture::new(false).await;
    f.counter.blocking.store(true, Ordering::SeqCst);
    let first = f.runtime();
    f.accept(&first).await;
    let id = f.child.execution_id();
    let running = tokio::spawn(async move { Box::pin(first.resume(id)).await });
    f.counter.entered.notified().await;
    let observed = Box::pin(f.runtime().resume(id)).await.unwrap();
    assert_eq!(observed.task.status.state, TaskState::Working);
    assert!(matches!(
        observed.execution.unwrap().status,
        GovernedProviderStatus::InFlight { .. }
    ));
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    f.counter.release.add_permits(1);
    assert_eq!(
        running.await.unwrap().unwrap().task.status.state,
        TaskState::Completed
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_forged_terminal_task_projection_cannot_certify_a_provider_outcome() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    f.accept(&runtime).await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::A2aTask,
        f.child.execution_id().to_string(),
    );
    let original = f.state.get(&key).await.unwrap().unwrap();
    let mut task: acteon_core::Task = serde_json::from_str(&original).unwrap();
    task.status.state = TaskState::Completed;
    f.state
        .set(&key, &serde_json::to_string(&task).unwrap(), None)
        .await
        .unwrap();
    assert!(
        Box::pin(runtime.resume(f.child.execution_id()))
            .await
            .is_err()
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    f.state.set(&key, &original, None).await.unwrap();
    Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap();
    let mut task: acteon_core::Task =
        serde_json::from_str(&f.state.get(&key).await.unwrap().unwrap()).unwrap();
    task.status.state = TaskState::Failed;
    f.state
        .set(&key, &serde_json::to_string(&task).unwrap(), None)
        .await
        .unwrap();
    assert!(
        Box::pin(runtime.resume(f.child.execution_id()))
            .await
            .is_err()
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn recipient_acceptance_can_queue_behind_the_senders_single_active_slot() {
    use acteon_governance::{AttemptStatus, StartRegistration, permit::PermittedAttempt};
    let f = Fixture::new(false).await;
    let ingress = AcceptedEffect {
        operation: "agent.invoke".into(),
        resources: std::iter::once(resource(ResourceKind::Agent, "notifier"))
            .chain(f.bound.effect().resources.iter().cloned())
            .collect(),
    };
    let StartRegistration::New(sender) = f
        .coordinator
        .register_permitted_attempt(PermittedAttempt {
            id: "sender",
            context: &f.parent,
            permits: &permits("caller"),
            effect: &ingress,
            request_digest: f.parent.reference().unwrap().request_digest(),
            units: 1,
            clock: f.clock.as_ref(),
        })
        .await
        .unwrap()
    else {
        panic!("new sender attempt")
    };
    let runtime = f.runtime();
    let message = TaskMessage::text("request-2", TaskRole::User, "A queued notification");
    let action = runtime.prepare_message(&message).unwrap();
    let mut recipient = admission(&f.coordinator, "worker", vec![f.bound.effect().clone()]).await;
    recipient.binding.request_digest = governed_provider_input_digest(&action).unwrap();
    let binding = Fixture::binding(&f.card, &f.bound);
    let child = f
        .contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "queued-notification",
            parent: &f.parent,
            parent_permits: &permits("caller"),
            grant: reference("service"),
            binding_digest: binding.digest(),
            recipient,
            recipient_permits: &permits("worker"),
            credential: credential("worker"),
            representation: None,
            intent_effects: vec![f.bound.effect().clone()],
            onward_grants: vec![],
            limits: limits(),
            clock: f.clock.as_ref(),
        })
        .await
        .unwrap();
    let accepted = runtime
        .accept(&child, &permits("worker"), &message)
        .await
        .unwrap();
    assert_eq!(accepted.status.state, TaskState::Submitted);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    let occupied = f.coordinator.snapshot().await.unwrap();
    assert_eq!(occupied.starts.len(), 1);
    assert_eq!(
        occupied.roots[&f.parent.execution_id().to_string()].active_attempts,
        1
    );
    assert_eq!(
        occupied.roots[&child.execution_id().to_string()].active_attempts,
        0
    );
    assert!(
        Box::pin(runtime.resume(child.execution_id()))
            .await
            .is_err()
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    f.coordinator
        .settle("sender", &sender.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let completed = Box::pin(runtime.resume(child.execution_id()))
        .await
        .unwrap();
    assert_eq!(completed.task.status.state, TaskState::Completed);
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    let final_state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        final_state.roots[&f.parent.execution_id().to_string()].spent_units,
        2
    );
    assert_eq!(
        final_state.roots[&f.parent.execution_id().to_string()].active_attempts,
        0
    );
}

#[tokio::test]
async fn queue_admission_still_refuses_current_revocation_and_exhausted_units() {
    use acteon_governance::{AttemptStatus, StartRegistration, permit::PermittedAttempt};
    for reason in ["offboard", "units"] {
        let f = Fixture::new(false).await;
        if reason == "offboard" {
            f.coordinator
                .change(
                    "offboard",
                    AuthorityChange::RevokeSubject {
                        subject: "caller".into(),
                    },
                    "operator",
                    "offboard",
                )
                .await
                .unwrap();
        } else {
            let ingress = AcceptedEffect {
                operation: "agent.invoke".into(),
                resources: std::iter::once(resource(ResourceKind::Agent, "notifier"))
                    .chain(f.bound.effect().resources.iter().cloned())
                    .collect(),
            };
            for ordinal in 0..3 {
                let id = format!("consume-{ordinal}");
                let StartRegistration::New(start) = f
                    .coordinator
                    .register_permitted_attempt(PermittedAttempt {
                        id: &id,
                        context: &f.parent,
                        permits: &permits("caller"),
                        effect: &ingress,
                        request_digest: f.parent.reference().unwrap().request_digest(),
                        units: 1,
                        clock: f.clock.as_ref(),
                    })
                    .await
                    .unwrap()
                else {
                    panic!("new attempt")
                };
                f.coordinator
                    .settle(&id, &start.token, AttemptStatus::Settled)
                    .await
                    .unwrap();
            }
        }
        assert!(
            f.runtime()
                .accept(&f.child, &permits("worker"), &f.message)
                .await
                .is_err()
        );
        assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
        assert!(
            f.state
                .get(&StateKey::new(
                    "city",
                    "tenant",
                    KeyKind::A2aTask,
                    f.child.execution_id().to_string()
                ))
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn copied_acceptance_and_substituted_task_identity_are_refused() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    let task = f.accept(&runtime).await;
    let acceptance_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(ACCEPTANCE_KIND.into()),
        &task.id,
    );
    let copied_id = uuid::Uuid::new_v4();
    let copied_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(ACCEPTANCE_KIND.into()),
        copied_id.to_string(),
    );
    let raw = f.state.get(&acceptance_key).await.unwrap().unwrap();
    f.state.set(&copied_key, &raw, None).await.unwrap();
    assert!(Box::pin(runtime.resume(copied_id)).await.is_err());
    let mut forged_acceptance: serde_json::Value = serde_json::from_str(&raw).unwrap();
    forged_acceptance["initial_task"]["metadata"]["acteon_governed_execution"]["execution_id"] =
        serde_json::json!(copied_id);
    f.state
        .set(&acceptance_key, &forged_acceptance.to_string(), None)
        .await
        .unwrap();
    assert!(
        Box::pin(runtime.resume(f.child.execution_id()))
            .await
            .is_err()
    );
    f.state.set(&acceptance_key, &raw, None).await.unwrap();
    let key = StateKey::new("city", "tenant", KeyKind::A2aTask, &task.id);
    for field in ["id", "namespace", "tenant", "contextId"] {
        let mut forged = serde_json::to_value(&task).unwrap();
        forged[field] = serde_json::json!("substituted");
        f.state.set(&key, &forged.to_string(), None).await.unwrap();
        assert!(
            Box::pin(runtime.resume(f.child.execution_id()))
                .await
                .is_err(),
            "{field}"
        );
        assert!(
            runtime
                .accept(&f.child, &permits("worker"), &f.message)
                .await
                .is_err(),
            "{field}"
        );
    }
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn terminal_result_is_repaired_from_execution_evidence_on_resume_and_accept_replay() {
    let f = Fixture::new(false).await;
    let runtime = f.runtime();
    let task = f.accept(&runtime).await;
    let original = Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap()
        .task;
    let key = StateKey::new("city", "tenant", KeyKind::A2aTask, &task.id);
    for accept_replay in [false, true] {
        let mut forged = original.clone();
        forged.artifacts[0].parts = vec![acteon_core::TaskPart::data(
            serde_json::json!({"forged":true}),
        )];
        f.state
            .set(&key, &serde_json::to_string(&forged).unwrap(), None)
            .await
            .unwrap();
        let repaired = if accept_replay {
            f.accept(&runtime).await
        } else {
            Box::pin(runtime.resume(f.child.execution_id()))
                .await
                .unwrap()
                .task
        };
        assert_eq!(
            serde_json::to_value(&repaired.artifacts).unwrap(),
            serde_json::to_value(&original.artifacts).unwrap()
        );
    }
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn removing_projection_metadata_does_not_let_the_reaper_certify_uncertain_work() {
    let f = Fixture::new(true).await;
    let runtime = f.runtime();
    let task = f.accept(&runtime).await;
    let mut unresolved = Box::pin(runtime.resume(f.child.execution_id()))
        .await
        .unwrap()
        .task;
    unresolved.metadata.clear();
    let key = StateKey::new("city", "tenant", KeyKind::A2aTask, &task.id);
    f.state
        .set(&key, &serde_json::to_string(&unresolved).unwrap(), None)
        .await
        .unwrap();
    let later = f.clock.now() + chrono::Duration::days(8);
    assert!(
        TaskEngine::new(f.state.clone())
            .fail_if_stale(&TaskScope::new("city", "tenant"), &task.id, later)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&f.parent.execution_id().to_string()]
            .active_attempts,
        1
    );
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated UUID prefix"]
async fn independent_redis_runtime_recovers_lost_acceptance_and_known_or_uncertain_results() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    for ambiguous in [false, true] {
        let settings = RedisConfig {
            url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
            prefix: format!("agent-runtime-{}", uuid::Uuid::new_v4()),
            ..Default::default()
        };
        let primary: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
        let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&settings).unwrap());
        let f = Fixture::new_with_state(Arc::new(FaultStore::new(primary)), ambiguous).await;
        let coordinator = AuthorityCoordinator::connect(peer.clone(), "city", "tenant")
            .await
            .unwrap();
        let contexts = Arc::new(
            TrustedContextStore::new(
                peer.clone(),
                coordinator.clone(),
                "city-domain".into(),
                "key".into(),
                vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
            )
            .unwrap(),
        );
        let replacement = Fixture::create_runtime(
            peer.clone(),
            coordinator.clone(),
            contexts.clone(),
            f.clock.clone(),
            &f.card,
            &f.bound,
        );
        let runtime = f.runtime();
        f.state
            .fail_next(
                KeyKind::Custom(ACCEPTANCE_KIND.into()),
                WriteOperation::CheckAndSet,
                FaultTiming::After,
            )
            .unwrap();
        assert!(
            runtime
                .accept(&f.child, &permits("worker"), &f.message)
                .await
                .is_err()
        );
        let recovered = contexts
            .recover_reference_for_observation(&f.child.reference().unwrap())
            .await
            .unwrap();
        let task = replacement
            .accept(&recovered, &permits("worker"), &f.message)
            .await
            .unwrap();
        assert_eq!(task.id, f.child.execution_id().to_string());
        Box::pin(runtime.resume(f.child.execution_id()))
            .await
            .unwrap();
        let replay = Box::pin(replacement.resume(f.child.execution_id()))
            .await
            .unwrap();
        let expected = if ambiguous {
            TaskState::Working
        } else {
            TaskState::Completed
        };
        assert_eq!(replay.task.status.state, expected);
        assert_eq!(f.counter.calls.load(Ordering::SeqCst), 1);
        let snapshot = coordinator.snapshot().await.unwrap();
        assert_eq!(snapshot.roots.len(), 2);
        assert_eq!(
            snapshot.roots[&f.parent.execution_id().to_string()].spent_units,
            1
        );
        assert_eq!(
            snapshot.roots[&f.parent.execution_id().to_string()].active_attempts,
            u64::from(ambiguous)
        );
        if ambiguous {
            assert!(matches!(
                replay.execution.unwrap().status,
                GovernedProviderStatus::ReconciliationRequired { .. }
            ));
        }
    }
}
