//! Service discovery composes real signed source grants with independent recipient authority.
use acteon_core::{
    Agent, AgentCard, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef, Skill,
    bus_agent_card::Interface,
};
use acteon_executor::delegation::{
    ApprovedPeerBinding, ApprovedPeerRegistry, ApprovedServicePlan, PeerCandidateQuery,
    PeerDiscoveryError, PeerRecipientResolver, RecipientDiscoveryContext,
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
    atomic::{AtomicUsize, Ordering},
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
struct Fixture {
    store: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: TrustedContextStore,
    clock: ManualClock,
    parent: VerifiedExecutionContext,
    recipient: VerifiedExecutionContext,
    grant: DelegationGrant,
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
        let mut agent = Agent::new("responder", "city", "tenant");
        agent.has_agent_card = true;
        agent.last_heartbeat_at = Some(clock.now());
        store
            .set(
                &StateKey::new("city", "tenant", KeyKind::BusAgent, "responder"),
                &serde_json::to_string(&agent).unwrap(),
                None,
            )
            .await
            .unwrap();
        store
            .set(
                &StateKey::new("city", "tenant", KeyKind::BusAgentCard, "responder"),
                &serde_json::to_string(&approved_card).unwrap(),
                None,
            )
            .await
            .unwrap();
        let registry =
            ApprovedPeerRegistry::new_trusted(store.clone(), "city", "tenant", vec![binding])
                .unwrap();
        Self {
            store,
            coordinator,
            contexts,
            clock,
            parent,
            recipient,
            grant,
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
