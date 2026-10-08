//! Service discovery composes real signed source grants with independent recipient authority.
use acteon_core::{
    Agent, AgentCard, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef, Skill, Task,
    TaskMessage, TaskRole, TaskState, bus_agent_card::Interface,
};
use acteon_executor::delegation::{
    ApprovedPeerBinding, ApprovedPeerRegistry, ApprovedServicePlan, DurablePeerTransport,
    PeerCancelDisposition, PeerCancelStatus, PeerCandidateQuery, PeerDiscoveryError,
    PeerRecipientResolver, PeerSendDisposition, PeerSendRequest, PeerSendStatus,
    PeerSubmissionCapability, PeerTaskRequest, PeerTransportAdapter, PeerTransportDependencies,
    PeerTransportError, RecipientDiscoveryContext,
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
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference, PermittedAttempt},
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
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
fn provider() -> AcceptedEffect {
    AcceptedEffect {
        operation: "provider.execute".into(),
        resources: vec![resource(ResourceKind::Provider, "pager")],
    }
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

fn ingress() -> AcceptedEffect {
    AcceptedEffect {
        operation: "agent.invoke".into(),
        resources: vec![
            resource(ResourceKind::Agent, "responder"),
            provider().resources[0].clone(),
        ],
    }
}
fn card() -> AgentCard {
    let mut card = AgentCard::new("responder", "city", "tenant", "Private service", "v1");
    card.skills.push(Skill::new("notify"));
    card.interfaces.push(Interface {
        kind: "rest".into(),
        url: "https://peer.example/a2a".into(),
    });
    card
}
fn live_agent(clock: &ManualClock) -> Agent {
    let mut agent = Agent::new("responder", "city", "tenant");
    agent.has_agent_card = true;
    agent.last_heartbeat_at = Some(clock.now());
    agent
}
async fn publish_registry(store: &dyn StateStore, agent: &Agent, card: &AgentCard) {
    for (kind, value) in [
        (KeyKind::BusAgent, serde_json::to_string(agent).unwrap()),
        (KeyKind::BusAgentCard, serde_json::to_string(card).unwrap()),
    ] {
        store
            .set(
                &StateKey::new("city", "tenant", kind, "responder"),
                &value,
                None,
            )
            .await
            .unwrap();
    }
}
fn binding(card: &AgentCard, direct: Vec<AcceptedEffect>) -> ApprovedPeerBinding {
    ApprovedPeerBinding::new_service_trusted(
        card,
        actor("worker"),
        "notify",
        "https://peer.example/a2a",
        "rest",
        ingress(),
        ApprovedServicePlan::new_trusted(vec![provider()], direct).unwrap(),
    )
    .unwrap()
}
fn registry(store: Arc<dyn StateStore>, binding: ApprovedPeerBinding) -> ApprovedPeerRegistry {
    ApprovedPeerRegistry::new_trusted(store, "city", "tenant", vec![binding]).unwrap()
}
struct Fixture {
    store: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: TrustedContextStore,
    clock: ManualClock,
    parent: VerifiedExecutionContext,
    recipient: VerifiedExecutionContext,
    grant: DelegationGrant,
    binding: ApprovedPeerBinding,
    registry: ApprovedPeerRegistry,
}
impl Fixture {
    async fn new() -> Self {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let coordinator = AuthorityCoordinator::initialize(
            store.clone(),
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
        let clock = ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap());
        let contexts = TrustedContextStore::new(
            store.clone(),
            coordinator.clone(),
            "city-domain".into(),
            "key".into(),
            vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
        )
        .unwrap();
        publish_actor(&coordinator, "caller", vec![ingress()]).await;
        publish_actor(&coordinator, "worker", vec![provider()]).await;
        let approved_card = card();
        let binding = binding(&approved_card, vec![provider()]);
        let grant = DelegationGrant {
            id: "service".into(),
            revision: 1,
            source: actor("caller"),
            target: actor("worker"),
            agent_resource: resource(ResourceKind::Agent, "responder"),
            binding_digest: binding.digest().into(),
            skill: "notify".into(),
            ingress_effect: ingress(),
            effects: vec![provider()],
            valid_from_ms: 0,
            limits: limits(),
            max_depth: 3,
        };
        publish_grant(&coordinator, grant.clone(), &clock).await;
        let parent = contexts
            .capture_delegating_root(DelegatingRootAdmission {
                admission_key: "caller-root",
                admission: admission(&coordinator, "caller", vec![ingress()]).await,
                permits: &permits("caller"),
                credential: credential("caller"),
                grants: vec![reference("service")],
                limits: limits(),
                representation: None,
                clock: &clock,
            })
            .await
            .unwrap();
        let recipient = contexts
            .capture_credentialed_root(
                admission(&coordinator, "worker", vec![provider()]).await,
                &permits("worker"),
                credential("worker"),
                limits(),
                &clock,
            )
            .await
            .unwrap();
        publish_registry(store.as_ref(), &live_agent(&clock), &approved_card).await;
        let registry = registry(store.clone(), binding.clone());
        Self {
            store,
            coordinator,
            contexts,
            clock,
            parent,
            recipient,
            grant,
            binding,
            registry,
        }
    }
    async fn retire(&self) {
        self.coordinator
            .revoke_delegation_grant(
                "retire",
                "service",
                1,
                &issuer(&self.grant),
                &self.coordinator.snapshot().await.unwrap().stamp(),
                "stop service",
                &self.clock,
            )
            .await
            .unwrap();
    }
    async fn discover(
        &self,
        parent: &VerifiedExecutionContext,
        resolver: &Resolver<'_>,
    ) -> Vec<acteon_executor::delegation::PeerCandidate> {
        self.registry
            .discover_candidates(
                "notify",
                PeerCandidateQuery {
                    coordinator: &self.coordinator,
                    parent,
                    parent_permits: &permits("caller"),
                    recipients: resolver,
                    clock: &self.clock,
                },
            )
            .await
            .unwrap()
    }
}
struct Resolver<'a> {
    fixture: &'a Fixture,
    calls: AtomicUsize,
    retire: bool,
    expire: bool,
}
impl<'a> Resolver<'a> {
    fn new(fixture: &'a Fixture) -> Self {
        Self {
            fixture,
            calls: AtomicUsize::new(0),
            retire: false,
            expire: false,
        }
    }
}
#[async_trait::async_trait]
impl PeerRecipientResolver for Resolver<'_> {
    async fn resolve(
        &self,
        _: &str,
        _: &str,
        target: &PrincipalIdentity,
    ) -> Result<Option<RecipientDiscoveryContext>, PeerDiscoveryError> {
        assert_eq!(target, &actor("worker"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.retire {
            self.fixture.retire().await;
        }
        if self.expire {
            self.fixture
                .clock
                .advance_to(std::time::Duration::from_secs(20))
                .unwrap();
        }
        Ok(Some(RecipientDiscoveryContext {
            context: self.fixture.recipient.clone(),
            permits: permits("worker"),
        }))
    }
}
#[tokio::test]
async fn service_discovery_checks_ingress_and_private_effects_without_borrowing_permission() {
    let f = Fixture::new().await;
    let resolver = Resolver::new(&f);
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    f.coordinator
        .check_service_delegation_source(
            &f.parent,
            &permits("caller"),
            acteon_governance::delegation::ServiceDiscoveryBinding {
                target: &f.grant.target,
                agent_resource: &f.grant.agent_resource,
                binding_digest: &f.grant.binding_digest,
                skill: &f.grant.skill,
                ingress: &f.grant.ingress_effect,
                intent: &f.grant.effects,
                direct_effects: &f.grant.effects,
            },
            &f.clock,
        )
        .await
        .unwrap();
    let candidates = f.discover(&f.parent, &resolver).await;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].agent_id, "responder");
    assert_eq!(candidates[0].target, actor("worker"));
    assert_eq!(candidates[0].accepted_grant, Some(reference("service")));
    assert_eq!(
        serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap(),
        before
    );
    assert!(
        f.coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "cannot-borrow",
                context: &f.parent,
                permits: &permits("caller"),
                effect: &provider(),
                request_digest: f.parent.reference().unwrap().request_digest(),
                units: 1,
                clock: &f.clock
            })
            .await
            .is_err()
    );
    // Actual new input requires a separately authenticated recipient admission.
    let mut recipient = admission(&f.coordinator, "worker", vec![provider()]).await;
    recipient.binding.request_digest = "e".repeat(64);
    let child = f
        .contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "selected-service",
            parent: &f.parent,
            parent_permits: &permits("caller"),
            grant: candidates[0].accepted_grant.clone().unwrap(),
            binding_digest: &candidates[0].binding_digest,
            recipient,
            recipient_permits: &permits("worker"),
            credential: credential("worker"),
            representation: None,
            intent_effects: vec![provider()],
            onward_grants: vec![],
            limits: limits(),
            clock: &f.clock,
        })
        .await
        .unwrap();
    assert_ne!(child.execution_id(), f.recipient.execution_id());
    let start = f
        .coordinator
        .register_permitted_attempt(PermittedAttempt {
            id: "private-effect",
            context: &child,
            permits: &permits("worker"),
            effect: &provider(),
            request_digest: child.reference().unwrap().request_digest(),
            units: 1,
            clock: &f.clock,
        })
        .await
        .unwrap();
    assert!(matches!(
        start,
        acteon_governance::StartRegistration::New(_)
    ));
    let state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        state.roots[&f.parent.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(
        state.roots[&child.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(
        state.roots[&f.recipient.execution_id().to_string()].spent_units,
        0
    );
}

struct TransportAdapter {
    digest: String,
    capability: PeerSubmissionCapability,
    outcome: AtomicU8,
    calls: AtomicUsize,
    observation_calls: AtomicUsize,
    cancellation_calls: AtomicUsize,
    observed: std::sync::Mutex<Option<Task>>,
}
impl TransportAdapter {
    fn new(
        binding: &ApprovedPeerBinding,
        capability: PeerSubmissionCapability,
        outcome: u8,
    ) -> Self {
        Self {
            digest: binding.digest().into(),
            capability,
            outcome: AtomicU8::new(outcome),
            calls: AtomicUsize::new(0),
            observation_calls: AtomicUsize::new(0),
            cancellation_calls: AtomicUsize::new(0),
            observed: std::sync::Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl PeerTransportAdapter for TransportAdapter {
    fn revision(&self) -> &'static str {
        "test-a2a-v1"
    }
    fn binding_digest(&self) -> &str {
        &self.digest
    }
    fn submission_capability(&self) -> PeerSubmissionCapability {
        self.capability
    }
    async fn send(
        &self,
        request: PeerSendRequest<'_>,
    ) -> Result<PeerSendDisposition, PeerTransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.endpoint, "https://peer.example/a2a");
        assert_eq!(request.transport, "rest");
        assert_eq!(request.message.message_id, "peer-message");
        if self.outcome.load(Ordering::SeqCst) == 4 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            return Ok(PeerSendDisposition::Uncertain);
        }
        match self.outcome.load(Ordering::SeqCst) {
            0 => {
                let task = Task::new(
                    "remote-task",
                    request.parent.namespace(),
                    request.parent.tenant(),
                );
                *self.observed.lock().unwrap() = Some(task.clone());
                Ok(PeerSendDisposition::Accepted {
                    task: Box::new(task),
                    source_context: request.parent.clone(),
                })
            }
            1 => Ok(PeerSendDisposition::Uncertain),
            2 => Ok(PeerSendDisposition::Rejected {
                code: "peer_denied".into(),
            }),
            _ => Ok(PeerSendDisposition::Accepted {
                task: Box::new(Task::new(
                    "remote-task",
                    request.parent.namespace(),
                    "wrong-tenant",
                )),
                source_context: request.parent.clone(),
            }),
        }
    }

    async fn observe_task(&self, request: PeerTaskRequest<'_>) -> Result<Task, PeerTransportError> {
        self.observation_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.endpoint, "https://peer.example/a2a");
        assert_eq!(request.transport, "rest");
        assert_eq!(request.task_id, "remote-task");
        assert_eq!(request.source_context.namespace(), "city");
        let mut task = self
            .observed
            .lock()
            .unwrap()
            .clone()
            .ok_or(PeerTransportError::Unavailable)?;
        let outcome = self.outcome.load(Ordering::SeqCst);
        match outcome {
            5 => task.transition_to(TaskState::Working, None).unwrap(),
            6 => task.transition_to(TaskState::Completed, None).unwrap(),
            7 => task.id = "substituted-task".into(),
            _ => {}
        }
        if outcome != 7 {
            *self.observed.lock().unwrap() = Some(task.clone());
        }
        Ok(task)
    }

    async fn cancel_task(
        &self,
        request: PeerTaskRequest<'_>,
    ) -> Result<PeerCancelDisposition, PeerTransportError> {
        self.cancellation_calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.task_id, "remote-task");
        assert_eq!(request.source_context.namespace(), "city");
        match self.outcome.load(Ordering::SeqCst) {
            8 => {
                let mut task = self.observed.lock().unwrap().clone().unwrap();
                task.transition_to(TaskState::Canceled, None).unwrap();
                *self.observed.lock().unwrap() = Some(task.clone());
                Ok(PeerCancelDisposition::Final {
                    task: Box::new(task),
                })
            }
            9 => Ok(PeerCancelDisposition::Uncertain),
            10 => Ok(PeerCancelDisposition::Unsupported),
            11 => Ok(PeerCancelDisposition::Rejected {
                code: "remote_cancel_denied".into(),
            }),
            12 => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Ok(PeerCancelDisposition::Uncertain)
            }
            _ => Err(PeerTransportError::Unavailable),
        }
    }
}

fn peer_message(text: &str) -> TaskMessage {
    TaskMessage::text("peer-message", TaskRole::User, text)
}

fn peer_transport(f: &Fixture, adapter: Arc<dyn PeerTransportAdapter>) -> DurablePeerTransport {
    DurablePeerTransport::new_trusted(
        PeerTransportDependencies {
            state: f.store.clone(),
            coordinator: f.coordinator.clone(),
            clock: Arc::new(f.clock.clone()),
            encryptor: None,
        },
        adapter,
        std::time::Duration::from_secs(1),
    )
    .unwrap()
}

#[tokio::test]
async fn durable_peer_submission_sends_once_and_conflicts_on_changed_input() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let receipt = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    assert!(matches!(receipt.status, PeerSendStatus::Accepted { .. }));
    let replay = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    assert_eq!(replay.submission_id, receipt.submission_id);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let observed = transport
        .observe(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    assert_eq!(observed.submission_id, receipt.submission_id);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        transport
            .submit(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &peer_message("changed")
            )
            .await,
        Err(PeerTransportError::Conflict)
    ));
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_executor::delegation::transport::PEER_SEND_KIND.into()),
        receipt.submission_id.to_string(),
    );
    let mut corrupted: serde_json::Value =
        serde_json::from_str(&f.store.get(&key).await.unwrap().unwrap()).unwrap();
    corrupted["state"]["task"]["tenant"] = "other".into();
    f.store
        .set(&key, &corrupted.to_string(), None)
        .await
        .unwrap();
    assert!(matches!(
        transport
            .submit(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &peer_message("diagnose")
            )
            .await,
        Err(PeerTransportError::Conflict)
    ));
}

#[tokio::test]
async fn remote_task_refresh_rechecks_authority_and_journals_forward_progress() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let receipt = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    adapter.outcome.store(5, Ordering::SeqCst);
    let refreshed = transport
        .refresh_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            receipt.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(
        refreshed.status,
        PeerSendStatus::Accepted { task, .. } if task.status.state == TaskState::Working
    ));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.observation_calls.load(Ordering::SeqCst), 1);

    adapter.outcome.store(6, Ordering::SeqCst);
    let terminal = transport
        .refresh_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            receipt.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(
        terminal.status,
        PeerSendStatus::Accepted { task, .. } if task.status.state == TaskState::Completed
    ));
    let terminal_again = transport
        .refresh_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            receipt.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(
        terminal_again.status,
        PeerSendStatus::Accepted { task, .. } if task.status.state == TaskState::Completed
    ));
    assert_eq!(adapter.observation_calls.load(Ordering::SeqCst), 2);

    assert!(matches!(
        transport
            .refresh_task(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                uuid::Uuid::new_v4(),
            )
            .await,
        Err(PeerTransportError::Conflict)
    ));
    f.retire().await;
    assert!(matches!(
        transport
            .refresh_task(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                receipt.submission_id,
            )
            .await,
        Err(PeerTransportError::Refused)
    ));
    assert_eq!(adapter.observation_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn remote_task_refresh_rejects_identity_substitution_without_overwriting_snapshot() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let accepted = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    adapter.outcome.store(7, Ordering::SeqCst);
    assert!(matches!(
        transport
            .refresh_task(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                accepted.submission_id,
            )
            .await,
        Err(PeerTransportError::Unavailable)
    ));
    adapter.outcome.store(0, Ordering::SeqCst);
    let retained = transport
        .refresh_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            accepted.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(
        retained.status,
        PeerSendStatus::Accepted { task, .. }
            if task.id == "remote-task" && task.status.state == TaskState::Submitted
    ));
}

#[tokio::test]
async fn remote_cancel_is_durable_at_most_once_and_projects_terminal_task() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let accepted = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    adapter.outcome.store(8, Ordering::SeqCst);
    let canceled = transport
        .cancel_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            accepted.submission_id,
        )
        .await
        .unwrap();
    assert_eq!(
        canceled.cancellation_id,
        uuid::Uuid::new_v5(&accepted.submission_id, b"acteon.peer-cancel.v1")
    );
    assert!(matches!(
        canceled.status,
        PeerCancelStatus::Reconciled { task } if task.status.state == TaskState::Canceled
    ));
    let replay = transport
        .cancel_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            accepted.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(replay.status, PeerCancelStatus::Reconciled { .. }));
    assert_eq!(adapter.cancellation_calls.load(Ordering::SeqCst), 1);
    let projected = transport
        .refresh_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            accepted.submission_id,
        )
        .await
        .unwrap();
    assert!(matches!(
        projected.status,
        PeerSendStatus::Accepted { task, .. } if task.status.state == TaskState::Canceled
    ));
    assert_eq!(adapter.observation_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_remote_cancel_claims_exactly_one_delivery() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let accepted = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    adapter.outcome.store(12, Ordering::SeqCst);
    let parent_permits = permits("caller");
    let cancel = || {
        transport.cancel_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &parent_permits,
            accepted.submission_id,
        )
    };
    let (first, second) = tokio::join!(cancel(), cancel());
    assert!(matches!(first.unwrap().status, PeerCancelStatus::Uncertain));
    assert!(matches!(
        second.unwrap().status,
        PeerCancelStatus::Uncertain
    ));
    assert_eq!(adapter.cancellation_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ambiguous_remote_cancel_is_never_resent_and_rechecks_current_authority() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let accepted = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &peer_message("diagnose"),
        )
        .await
        .unwrap();
    adapter.outcome.store(9, Ordering::SeqCst);
    let parent_permits = permits("caller");
    let cancel = || {
        transport.cancel_task(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &parent_permits,
            accepted.submission_id,
        )
    };
    assert!(matches!(
        cancel().await.unwrap().status,
        PeerCancelStatus::Uncertain
    ));
    assert!(matches!(
        cancel().await.unwrap().status,
        PeerCancelStatus::Uncertain
    ));
    adapter.outcome.store(5, Ordering::SeqCst);
    assert!(matches!(
        cancel().await.unwrap().status,
        PeerCancelStatus::Uncertain
    ));
    adapter.outcome.store(6, Ordering::SeqCst);
    assert!(matches!(
        cancel().await.unwrap().status,
        PeerCancelStatus::Reconciled { task } if task.status.state == TaskState::Completed
    ));
    assert_eq!(adapter.cancellation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter.observation_calls.load(Ordering::SeqCst), 3);
    f.retire().await;
    assert!(matches!(
        transport
            .cancel_task(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                accepted.submission_id,
            )
            .await,
        Err(PeerTransportError::Refused)
    ));
    assert_eq!(adapter.cancellation_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn remote_cancel_preserves_definitive_unsupported_and_rejected_outcomes() {
    for (outcome, expected) in [(10, "unsupported"), (11, "rejected")] {
        let f = Fixture::new().await;
        let adapter = Arc::new(TransportAdapter::new(
            &f.binding,
            PeerSubmissionCapability::AtMostOnce,
            0,
        ));
        let transport = peer_transport(&f, adapter.clone());
        let accepted = transport
            .submit(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &peer_message("diagnose"),
            )
            .await
            .unwrap();
        adapter.outcome.store(outcome, Ordering::SeqCst);
        let receipt = transport
            .cancel_task(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                accepted.submission_id,
            )
            .await
            .unwrap();
        assert!(match receipt.status {
            PeerCancelStatus::Unsupported => expected == "unsupported",
            PeerCancelStatus::Rejected { ref code } => {
                expected == "rejected" && code == "remote_cancel_denied"
            }
            _ => false,
        });
        assert_eq!(adapter.cancellation_calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn ambiguous_peer_submission_requires_explicit_qualified_replay() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::VerifiedIdempotent,
        1,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let message = peer_message("diagnose");
    let uncertain = transport
        .submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &message,
        )
        .await
        .unwrap();
    assert!(matches!(uncertain.status, PeerSendStatus::Uncertain));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    adapter.outcome.store(0, Ordering::SeqCst);
    let accepted = transport
        .replay_idempotent(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &permits("caller"),
            &message,
        )
        .await
        .unwrap();
    assert_eq!(accepted.submission_id, uncertain.submission_id);
    assert!(matches!(accepted.status, PeerSendStatus::Accepted { .. }));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn send_time_authority_and_at_most_once_ambiguity_fail_closed() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        1,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let message = peer_message("diagnose");
    assert!(matches!(
        transport
            .replay_idempotent(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &message
            )
            .await,
        Err(PeerTransportError::Refused)
    ));
    let mut suspended = live_agent(&f.clock);
    suspended.admin_state = acteon_core::AgentAdminState::Suspended;
    publish_registry(f.store.as_ref(), &suspended, &card()).await;
    assert!(matches!(
        transport
            .submit(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &message
            )
            .await,
        Err(PeerTransportError::Refused)
    ));
    publish_registry(f.store.as_ref(), &live_agent(&f.clock), &card()).await;
    f.retire().await;
    assert!(matches!(
        transport
            .submit(
                &f.registry,
                "responder",
                "notify",
                &f.parent,
                &permits("caller"),
                &message
            )
            .await,
        Err(PeerTransportError::Refused)
    ));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_submission_has_one_sender_and_malformed_acceptance_is_uncertain() {
    let f = Fixture::new().await;
    let adapter = Arc::new(TransportAdapter::new(
        &f.binding,
        PeerSubmissionCapability::AtMostOnce,
        0,
    ));
    let transport = peer_transport(&f, adapter.clone());
    let message = peer_message("diagnose");
    let parent_permits = permits("caller");
    let (left, right) = tokio::join!(
        transport.submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &parent_permits,
            &message
        ),
        transport.submit(
            &f.registry,
            "responder",
            "notify",
            &f.parent,
            &parent_permits,
            &message
        ),
    );
    assert!(left.is_ok() && right.is_ok());
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);

    let other = Fixture::new().await;
    let malformed = Arc::new(TransportAdapter::new(
        &other.binding,
        PeerSubmissionCapability::AtMostOnce,
        3,
    ));
    let receipt = peer_transport(&other, malformed)
        .submit(
            &other.registry,
            "responder",
            "notify",
            &other.parent,
            &permits("caller"),
            &message,
        )
        .await
        .unwrap();
    assert!(matches!(receipt.status, PeerSendStatus::Uncertain));

    let timeout_fixture = Fixture::new().await;
    let slow = Arc::new(TransportAdapter::new(
        &timeout_fixture.binding,
        PeerSubmissionCapability::AtMostOnce,
        4,
    ));
    let timeout_transport = DurablePeerTransport::new_trusted(
        PeerTransportDependencies {
            state: timeout_fixture.store.clone(),
            coordinator: timeout_fixture.coordinator.clone(),
            clock: Arc::new(timeout_fixture.clock.clone()),
            encryptor: None,
        },
        slow.clone(),
        std::time::Duration::from_millis(1),
    )
    .unwrap();
    let timed_out = timeout_transport
        .submit(
            &timeout_fixture.registry,
            "responder",
            "notify",
            &timeout_fixture.parent,
            &permits("caller"),
            &message,
        )
        .await
        .unwrap();
    assert!(matches!(timed_out.status, PeerSendStatus::Uncertain));
    assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn unaccepted_or_retired_service_grants_do_not_resolve_private_recipient() {
    let f = Fixture::new().await;
    let resolver = Resolver::new(&f);
    let ordinary = f
        .contexts
        .capture_credentialed_root(
            admission(&f.coordinator, "caller", vec![ingress()]).await,
            &permits("caller"),
            credential("caller"),
            limits(),
            &f.clock,
        )
        .await
        .unwrap();
    assert!(f.discover(&ordinary, &resolver).await.is_empty());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    f.retire().await;
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn retirement_during_private_resolution_is_rechecked_without_allocating() {
    let f = Fixture::new().await;
    let mut resolver = Resolver::new(&f);
    resolver.retire = true;
    let before = f.coordinator.snapshot().await.unwrap();
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    let after = f.coordinator.snapshot().await.unwrap();
    assert_eq!(
        serde_json::to_value(before.roots).unwrap(),
        serde_json::to_value(after.roots).unwrap()
    );
    assert!(after.starts.is_empty());
}
#[tokio::test]
async fn expiry_during_private_resolution_uses_current_clock() {
    let f = Fixture::new().await;
    let mut resolver = Resolver::new(&f);
    resolver.expire = true;
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}
#[tokio::test]
async fn recipient_revocation_and_source_offboarding_refuse_service_discovery() {
    for source in [false, true] {
        let f = Fixture::new().await;
        let resolver = Resolver::new(&f);
        f.coordinator
            .change(
                "offboard",
                AuthorityChange::RevokeSubject {
                    subject: if source { "caller" } else { "worker" }.into(),
                },
                "operator",
                "offboard",
            )
            .await
            .unwrap();
        assert!(f.discover(&f.parent, &resolver).await.is_empty());
        assert_eq!(resolver.calls.load(Ordering::SeqCst), usize::from(!source));
    }
}
#[test]
fn service_plan_approval_rejects_unqualified_effects_and_pins_direct_operations() {
    assert!(ApprovedServicePlan::new_trusted(vec![provider()], vec![ingress()]).is_err());
    assert!(
        ApprovedServicePlan::new_trusted(vec![provider(), provider()], vec![provider()]).is_err()
    );
    let approved_card = card();
    let base = ApprovedPeerBinding::new_trusted(
        &approved_card,
        actor("worker"),
        "notify",
        "https://peer.example/a2a",
        "rest",
        ingress(),
    )
    .unwrap();
    assert_ne!(
        binding(&approved_card, vec![provider()]).digest(),
        base.digest()
    );
    let outside = AcceptedEffect {
        operation: "provider.execute".into(),
        resources: vec![resource(ResourceKind::Provider, "outside")],
    };
    let plan = ApprovedServicePlan::new_trusted(vec![outside.clone()], vec![outside]).unwrap();
    assert!(
        ApprovedPeerBinding::new_service_trusted(
            &card(),
            actor("worker"),
            "notify",
            "https://peer.example/a2a",
            "rest",
            ingress(),
            plan
        )
        .is_err()
    );
}

#[tokio::test]
async fn narrowing_current_service_intent_refuses_private_resolution() {
    let f = Fixture::new().await;
    let resolver = Resolver::new(&f);
    let mut narrowed = f.grant.clone();
    narrowed.revision = 2;
    narrowed.effects[0].operation = "provider.inspect".into();
    publish_grant(&f.coordinator, narrowed, &f.clock).await;
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[test]
fn direct_operation_selection_is_part_of_service_binding_digest() {
    let approved_card = card();
    let execute = provider();
    let inspect = AcceptedEffect {
        operation: "provider.inspect".into(),
        resources: execute.resources.clone(),
    };
    let approve = |direct| {
        ApprovedPeerBinding::new_service_trusted(
            &approved_card,
            actor("worker"),
            "notify",
            "https://peer.example/a2a",
            "rest",
            ingress(),
            ApprovedServicePlan::new_trusted(vec![execute.clone(), inspect.clone()], direct)
                .unwrap(),
        )
        .unwrap()
    };
    let first = approve(vec![execute.clone()]);
    let second = approve(vec![inspect.clone()]);
    assert_ne!(first.digest(), second.digest());
    assert_eq!(first.service_plan().unwrap().direct_effects(), &[execute]);
    assert_eq!(first.service_plan().unwrap().intent().len(), 2);
}

#[tokio::test]
async fn approved_service_discovery_reads_actual_card_instead_of_presence_hint() {
    let f = Fixture::new().await;
    let agent_key = StateKey::new("city", "tenant", KeyKind::BusAgent, "responder");
    let card_key = StateKey::new("city", "tenant", KeyKind::BusAgentCard, "responder");
    let mut agent: Agent =
        serde_json::from_str(&f.store.get(&agent_key).await.unwrap().unwrap()).unwrap();
    let approved_card = f.store.get(&card_key).await.unwrap().unwrap();
    agent.has_agent_card = false;
    f.store
        .set(&agent_key, &serde_json::to_string(&agent).unwrap(), None)
        .await
        .unwrap();
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    let resolver = Resolver::new(&f);
    assert_eq!(f.discover(&f.parent, &resolver).await.len(), 1);
    // Removing the actual card must hide the candidate even with a true hint.
    agent.has_agent_card = true;
    f.store
        .set(&agent_key, &serde_json::to_string(&agent).unwrap(), None)
        .await
        .unwrap();
    f.store.delete(&card_key).await.unwrap();
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    // A changed card cannot borrow approval from either hint value.
    let mut changed: AgentCard = serde_json::from_str(&approved_card).unwrap();
    changed.name = "different unapproved card".into();
    f.store
        .set(&card_key, &serde_json::to_string(&changed).unwrap(), None)
        .await
        .unwrap();
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    f.store.set(&card_key, &approved_card, None).await.unwrap();
    f.retire().await;
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    // Discovery itself never allocates or spends an execution attempt.
    let after = f.coordinator.snapshot().await.unwrap();
    let prior: serde_json::Value = before;
    assert_eq!(serde_json::to_value(&after.roots).unwrap(), prior["roots"]);
    assert!(after.starts.is_empty());
}

#[tokio::test]
async fn source_peer_options_expose_only_safe_reviewed_registry_fields() {
    let f = Fixture::new().await;
    let allowed = vec!["responder".to_owned()];
    let options = f
        .registry
        .discover_source_options(
            "notify",
            &allowed,
            &f.coordinator,
            &f.parent,
            &permits("caller"),
            &f.clock,
        )
        .await
        .unwrap();
    assert_eq!(options.len(), 1);
    let option = &options[0];
    assert_eq!(option.agent_id, "responder");
    assert_eq!(option.skill, "notify");
    assert_eq!(option.description_untrusted, None);
    assert_eq!(option.binding_digest, f.binding.digest());
    assert_eq!(option.checked_at_ms, f.clock.now().timestamp_millis());
    assert!(
        f.registry
            .discover_source_options(
                "notify",
                &["responder".into(), "responder".into()],
                &f.coordinator,
                &f.parent,
                &permits("caller"),
                &f.clock,
            )
            .await
            .is_err()
    );
    f.retire().await;
    let retired = f
        .registry
        .discover_source_options(
            "notify",
            &allowed,
            &f.coordinator,
            &f.parent,
            &permits("caller"),
            &f.clock,
        )
        .await
        .unwrap();
    assert_eq!(retired.len(), 0);
}

#[tokio::test]
async fn actual_approved_card_cannot_override_registry_retirement() {
    use acteon_governance::{
        control::{ControlChangeAuthorization, ControlChangeCeiling},
        registry::{AgentRegistryIssuanceCeiling, AgentRegistryQualification},
    };
    let f = Fixture::new().await;
    let qualification = AgentRegistryQualification {
        agent: f.grant.agent_resource.clone(),
        target: f.grant.target.clone(),
        revision: 1,
        bindings: std::collections::BTreeMap::from([(
            f.grant.skill.clone(),
            f.grant.binding_digest.clone(),
        )]),
    };
    let ceiling = AgentRegistryIssuanceCeiling {
        issuer: actor("operator"),
        approved: vec![qualification.clone()],
        valid_from_ms: 0,
        deadline_ms: 10_000,
    };
    f.coordinator
        .publish_agent_registry(
            "qualify-registry",
            qualification,
            0,
            &ceiling,
            &f.coordinator.snapshot().await.unwrap().stamp(),
            "reviewed registry",
            &f.clock,
        )
        .await
        .unwrap();
    let resolver = Resolver::new(&f);
    assert_eq!(f.discover(&f.parent, &resolver).await.len(), 1);
    let bounds = ControlChangeCeiling {
        actor: actor("operator"),
        subjects: vec![],
        resources: vec![f.grant.agent_resource.clone()],
        valid_from_ms: 0,
        deadline_ms: 10_000,
    };
    f.coordinator
        .change_evaluated(
            "retire-registry",
            AuthorityChange::RetireAgentRegistry {
                agent: f.grant.agent_resource.clone(),
                expected_revision: 1,
            },
            "retire reviewed epoch",
            ControlChangeAuthorization {
                ceiling: &bounds,
                evaluated_authority: &f.coordinator.snapshot().await.unwrap().stamp(),
                clock: &f.clock,
            },
        )
        .await
        .unwrap();
    assert!(f.discover(&f.parent, &resolver).await.is_empty());
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
}
