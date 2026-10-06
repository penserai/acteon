//! Real signed admission and start accounting; no external transport is mocked into authority.
use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AttemptStatus, AuthorityChange, AuthorityCoordinator, CoordinatorLimits, RootBudgetLimits,
    StartRegistration,
    context::{
        AcceptedEffect, ChildContextAdmission, ContextBinding, ContextSigningKey,
        DelegatedContextAdmission, DelegatingRootAdmission, ExecutionContextHandle,
        RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
    },
    credential::{CredentialAuthority, CredentialReference},
    delegation_policy::{
        DelegationGrant, DelegationGrantReference, DelegationIssuanceCeiling,
        EvaluatedDelegationPublication,
    },
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference, PermittedAttempt},
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use std::sync::Arc;

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
fn delete_effect() -> AcceptedEffect {
    AcceptedEffect {
        operation: "provider.delete".into(),
        resources: provider().resources,
    }
}
fn ingress(id: &str, extras: &[ResourceRef]) -> AcceptedEffect {
    AcceptedEffect {
        operation: "agent.invoke".into(),
        resources: std::iter::once(resource(ResourceKind::Agent, id))
            .chain(extras.iter().cloned())
            .collect(),
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
struct Fixture {
    coordinator: AuthorityCoordinator,
    contexts: TrustedContextStore,
    clock: ManualClock,
    parent: VerifiedExecutionContext,
    grant: DelegationGrant,
}
impl Fixture {
    async fn new() -> Self {
        Self::on(Arc::new(MemoryStateStore::new())).await
    }
    async fn on(store: Arc<dyn StateStore>) -> Self {
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
            store,
            coordinator.clone(),
            "city-domain".into(),
            "key".into(),
            vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
        )
        .unwrap();
        let ingress = ingress("worker", &provider().resources);
        publish_actor(&coordinator, "caller", vec![ingress.clone()]).await;
        publish_actor(&coordinator, "worker", vec![provider(), delete_effect()]).await;
        let grant = DelegationGrant {
            id: "service".into(),
            revision: 1,
            source: actor("caller"),
            target: actor("worker"),
            agent_resource: resource(ResourceKind::Agent, "worker"),
            binding_digest: "c".repeat(64),
            skill: "notify".into(),
            ingress_effect: ingress.clone(),
            effects: vec![provider()],
            valid_from_ms: 0,
            limits: limits(),
            max_depth: 3,
        };
        publish_grant(&coordinator, grant.clone(), &clock).await;
        let parent = contexts
            .capture_delegating_root(DelegatingRootAdmission {
                admission_key: "root",
                admission: admission(&coordinator, "caller", vec![ingress]).await,
                permits: &permits("caller"),
                credential: credential("caller"),
                grants: vec![reference("service")],
                limits: limits(),
                representation: None,
                clock: &clock,
            })
            .await
            .unwrap();
        Self {
            coordinator,
            contexts,
            clock,
            parent,
            grant,
        }
    }
    async fn child(
        &self,
        key: &str,
    ) -> Result<VerifiedExecutionContext, acteon_governance::context::ContextError> {
        self.contexts
            .capture_delegated_child(DelegatedContextAdmission {
                admission_key: key,
                parent: &self.parent,
                parent_permits: &permits("caller"),
                grant: reference("service"),
                binding_digest: &self.grant.binding_digest,
                recipient: admission(&self.coordinator, "worker", vec![provider()]).await,
                recipient_permits: &permits("worker"),
                credential: credential("worker"),
                representation: None,
                intent_effects: vec![provider()],
                onward_grants: vec![],
                limits: limits(),
                clock: &self.clock,
            })
            .await
    }
    async fn start(
        &self,
        context: &VerifiedExecutionContext,
        id: &str,
    ) -> Result<StartRegistration, acteon_governance::CoordinationError> {
        self.coordinator
            .register_permitted_attempt(PermittedAttempt {
                id,
                context,
                permits: &permits("worker"),
                effect: &provider(),
                request_digest: context.reference().unwrap().request_digest(),
                units: 1,
                clock: &self.clock,
            })
            .await
    }
}

#[tokio::test]
async fn service_delegate_preserves_identity_and_charges_shared_ancestry() {
    let f = Fixture::new().await;
    assert!(!f.parent.within_accepted_ceiling(&provider()));
    let child = f.child("request").await.unwrap();
    assert_eq!(child.principal(), &actor("worker"));
    assert_eq!(child.original_requester(), &actor("caller"));
    assert_eq!(child.immediate_delegator(), Some(&actor("caller")));
    assert_eq!(child.root_execution_id(), f.parent.execution_id());
    assert_eq!(child.delegation_depth(), 1);
    let replay = f.child("request").await.unwrap();
    assert_eq!(replay.reference().unwrap(), child.reference().unwrap());
    for i in 0..3 {
        let id = format!("effect-{i}");
        let StartRegistration::New(start) = f.start(&child, &id).await.unwrap() else {
            panic!("new start");
        };
        assert!(matches!(
            f.start(&child, &id).await.unwrap(),
            StartRegistration::Existing(_)
        ));
        let state = f.coordinator.snapshot().await.unwrap();
        assert_eq!(
            state.roots[&f.parent.execution_id().to_string()].spent_units,
            i + 1
        );
        assert_eq!(
            state.roots[&child.execution_id().to_string()].spent_units,
            i + 1
        );
        f.coordinator
            .settle(&id, &start.token, AttemptStatus::Settled)
            .await
            .unwrap();
    }
    assert!(f.start(&child, "exhausted").await.is_err());
    let state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(state.starts.len(), 3);
    assert_eq!(state.roots.len(), 2, "no phantom recipient root");
}

#[tokio::test]
async fn retired_grant_blocks_fresh_starts_and_cannot_be_republished() {
    let f = Fixture::new().await;
    let child = f.child("request").await.unwrap();
    f.coordinator
        .revoke_delegation_grant(
            "retire",
            "service",
            1,
            &issuer(&f.grant),
            &f.coordinator.snapshot().await.unwrap().stamp(),
            "retired",
            &f.clock,
        )
        .await
        .unwrap();
    assert!(f.start(&child, "forbidden").await.is_err());
    assert!(f.child("new-request").await.is_err());
    assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
    let grant = DelegationGrant {
        revision: 2,
        ..f.grant.clone()
    };
    assert!(
        f.coordinator
            .publish_delegation_grant(EvaluatedDelegationPublication {
                change_id: "resurrect",
                grant: grant.clone(),
                expected_revision: 1,
                ceiling: &issuer(&grant),
                evaluated_authority: &f.coordinator.snapshot().await.unwrap().stamp(),
                reason: "invalid resurrection",
                clock: &f.clock
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn source_recipient_closure_and_cancellation_changes_refuse_next_effect() {
    for change in [
        AuthorityChange::RevokeSubject {
            subject: "caller".into(),
        },
        AuthorityChange::RevokeSubject {
            subject: "worker".into(),
        },
        AuthorityChange::RevokeCredential {
            credential_id: "caller-key".into(),
            expected_revision: 1,
        },
        AuthorityChange::RevokeCredential {
            credential_id: "worker-key".into(),
            expected_revision: 1,
        },
        AuthorityChange::RevokePermit {
            permit_id: "caller-permit".into(),
            expected_revision: 1,
        },
        AuthorityChange::CloseResource {
            resource: resource(ResourceKind::Agent, "worker"),
        },
    ] {
        let f = Fixture::new().await;
        let child = f.child("request").await.unwrap();
        f.coordinator
            .change("deny", change, "operator", "intervention")
            .await
            .unwrap();
        assert!(f.start(&child, "forbidden").await.is_err());
        assert!(f.coordinator.snapshot().await.unwrap().starts.is_empty());
    }
    let f = Fixture::new().await;
    let child = f.child("request").await.unwrap();
    f.coordinator
        .change(
            "cancel",
            AuthorityChange::CancelExecution {
                execution_id: f.parent.execution_id().to_string(),
            },
            "operator",
            "cancel sponsor",
        )
        .await
        .unwrap();
    assert!(f.start(&child, "canceled").await.is_err());
}

#[tokio::test]
async fn same_actor_continuation_of_delegate_keeps_sponsorship() {
    let f = Fixture::new().await;
    let child = f.child("request").await.unwrap();
    let continuation = f
        .contexts
        .capture_child(ChildContextAdmission {
            admission_key: "step",
            parent: &child,
            handle: ExecutionContextHandle::new(),
            execution_id: uuid::Uuid::new_v4(),
            request_digest: "d".repeat(64),
            accepted_effects: vec![provider()],
            restrictions: vec![],
            permits: &permits("worker"),
            limits: limits(),
            clock: &f.clock,
        })
        .await
        .unwrap();
    f.start(&continuation, "step-effect").await.unwrap();
    let state = f.coordinator.snapshot().await.unwrap();
    for c in [&f.parent, &child, &continuation] {
        assert_eq!(state.roots[&c.execution_id().to_string()].spent_units, 1);
    }
    assert_eq!(continuation.original_requester(), &actor("caller"));
}

#[tokio::test]
async fn concurrent_delegates_share_one_sponsor_concurrency_slot() {
    let f = Fixture::new().await;
    let left = f.child("left").await.unwrap();
    let right = f.child("right").await.unwrap();
    let (a, b) = tokio::join!(
        f.start(&left, "left-effect"),
        f.start(&right, "right-effect")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let state = f.coordinator.snapshot().await.unwrap();
    assert_eq!(state.starts.len(), 1);
    assert_eq!(
        state.roots[&f.parent.execution_id().to_string()].active_attempts,
        1
    );
    assert_eq!(
        state.roots[&left.execution_id().to_string()].spent_units
            + state.roots[&right.execution_id().to_string()].spent_units,
        1
    );
}

#[tokio::test]
async fn input_binding_identity_and_grant_substitution_do_not_allocate() {
    let f = Fixture::new().await;
    let good = f.child("request").await.unwrap();
    for failure in ["input", "binding", "recipient", "intent"] {
        let mut recipient = admission(&f.coordinator, "worker", vec![provider()]).await;
        let mut intent = vec![provider()];
        if failure == "input" {
            recipient.binding.request_digest = "d".repeat(64);
        }
        if failure == "recipient" {
            recipient.binding.principal = actor("other-worker");
        }
        if failure == "intent" {
            intent.push(AcceptedEffect {
                operation: "provider.delete".into(),
                resources: provider().resources,
            });
        }
        let result = f
            .contexts
            .capture_delegated_child(DelegatedContextAdmission {
                admission_key: "request",
                parent: &f.parent,
                parent_permits: &permits("caller"),
                grant: reference("service"),
                binding_digest: if failure == "binding" {
                    "wrong"
                } else {
                    &f.grant.binding_digest
                },
                recipient,
                recipient_permits: &permits("worker"),
                credential: credential("worker"),
                representation: None,
                intent_effects: intent,
                onward_grants: vec![],
                limits: limits(),
                clock: &f.clock,
            })
            .await;
        assert!(result.is_err(), "{failure}");
    }
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 2);
    assert_eq!(
        f.contexts
            .recover_reference(&good.reference().unwrap(), 100)
            .await
            .unwrap()
            .principal(),
        &actor("worker")
    );
}

#[tokio::test]
async fn sponsor_budget_id_cannot_be_rebound_to_another_context_handle() {
    let f = Fixture::new().await;
    let child = f.child("first").await.unwrap();
    let mut recipient = admission(&f.coordinator, "worker", vec![provider()]).await;
    recipient.binding.execution_id = child.execution_id();
    let result = f
        .contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "different-admission",
            parent: &f.parent,
            parent_permits: &permits("caller"),
            grant: reference("service"),
            binding_digest: &f.grant.binding_digest,
            recipient,
            recipient_permits: &permits("worker"),
            credential: credential("worker"),
            representation: None,
            intent_effects: vec![provider()],
            onward_grants: vec![],
            limits: limits(),
            clock: &f.clock,
        })
        .await;
    assert!(result.is_err());
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 2);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one nested service contract with independent actor credentials"
)]
async fn nested_services_preserve_root_intent_without_granting_direct_private_effects() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
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
    let clock = ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap());
    let contexts = TrustedContextStore::new(
        state,
        coordinator.clone(),
        "city-domain".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
    )
    .unwrap();
    let inner = ingress("peer", &provider().resources);
    let outer = ingress("worker", &inner.resources);
    publish_actor(&coordinator, "caller", vec![outer.clone()]).await;
    publish_actor(&coordinator, "worker", vec![inner.clone()]).await;
    publish_actor(&coordinator, "peer", vec![provider()]).await;
    let outer_grant = DelegationGrant {
        id: "outer".into(),
        revision: 1,
        source: actor("caller"),
        target: actor("worker"),
        agent_resource: resource(ResourceKind::Agent, "worker"),
        binding_digest: "c".repeat(64),
        skill: "coordinate".into(),
        ingress_effect: outer.clone(),
        effects: vec![inner.clone(), provider()],
        valid_from_ms: 0,
        limits: limits(),
        max_depth: 2,
    };
    let inner_grant = DelegationGrant {
        id: "inner".into(),
        revision: 1,
        source: actor("worker"),
        target: actor("peer"),
        agent_resource: resource(ResourceKind::Agent, "peer"),
        binding_digest: "d".repeat(64),
        skill: "notify".into(),
        ingress_effect: inner.clone(),
        effects: vec![provider()],
        valid_from_ms: 0,
        limits: limits(),
        max_depth: 2,
    };
    publish_grant(&coordinator, outer_grant.clone(), &clock).await;
    publish_grant(&coordinator, inner_grant.clone(), &clock).await;
    let root = contexts
        .capture_delegating_root(DelegatingRootAdmission {
            admission_key: "root",
            admission: admission(&coordinator, "caller", vec![outer]).await,
            permits: &permits("caller"),
            credential: credential("caller"),
            grants: vec![reference("outer")],
            limits: limits(),
            representation: None,
            clock: &clock,
        })
        .await
        .unwrap();
    let worker = contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "worker",
            parent: &root,
            parent_permits: &permits("caller"),
            grant: reference("outer"),
            binding_digest: &outer_grant.binding_digest,
            recipient: admission(&coordinator, "worker", vec![inner.clone()]).await,
            recipient_permits: &permits("worker"),
            credential: credential("worker"),
            representation: None,
            intent_effects: vec![inner, provider()],
            onward_grants: vec![reference("inner")],
            limits: limits(),
            clock: &clock,
        })
        .await
        .unwrap();
    assert!(!root.within_accepted_ceiling(&provider()));
    assert!(!worker.within_accepted_ceiling(&provider()));
    let peer = contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "peer",
            parent: &worker,
            parent_permits: &permits("worker"),
            grant: reference("inner"),
            binding_digest: &inner_grant.binding_digest,
            recipient: admission(&coordinator, "peer", vec![provider()]).await,
            recipient_permits: &permits("peer"),
            credential: credential("peer"),
            representation: None,
            intent_effects: vec![provider()],
            onward_grants: vec![],
            limits: limits(),
            clock: &clock,
        })
        .await
        .unwrap();
    assert_eq!(peer.delegation_depth(), 2);
    assert_eq!(peer.original_requester(), &actor("caller"));
    assert_eq!(peer.immediate_delegator(), Some(&actor("worker")));
    coordinator
        .register_permitted_attempt(PermittedAttempt {
            id: "nested-effect",
            context: &peer,
            permits: &permits("peer"),
            effect: &provider(),
            request_digest: peer.reference().unwrap().request_digest(),
            units: 1,
            clock: &clock,
        })
        .await
        .unwrap();
    let snapshot = coordinator.snapshot().await.unwrap();
    for c in [&root, &worker, &peer] {
        assert_eq!(snapshot.roots[&c.execution_id().to_string()].spent_units, 1);
    }
    coordinator
        .revoke_delegation_grant(
            "retire-outer",
            "outer",
            1,
            &issuer(&outer_grant),
            &coordinator.snapshot().await.unwrap().stamp(),
            "retired service",
            &clock,
        )
        .await
        .unwrap();
    assert!(
        coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "after-retirement",
                context: &peer,
                permits: &permits("peer"),
                effect: &provider(),
                request_digest: peer.reference().unwrap().request_digest(),
                units: 1,
                clock: &clock
            })
            .await
            .is_err()
    );
    assert_eq!(coordinator.snapshot().await.unwrap().starts.len(), 1);
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL"]
#[allow(
    clippy::too_many_lines,
    reason = "real independent-client persistence, authority and cleanup contract"
)]
async fn independent_redis_delegation_sponsorship_and_retirement() {
    use acteon_state::{KeyKind, StateKey};
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    use sha2::{Digest, Sha256};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("cross-principal-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let first: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let second: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let f = Fixture::on(first.clone()).await;
    let child = f.child("request").await.unwrap();
    let peer = AuthorityCoordinator::connect(second.clone(), "city", "tenant")
        .await
        .unwrap();
    let contexts = TrustedContextStore::new(
        second.clone(),
        peer.clone(),
        "city-domain".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![7; 32]).unwrap()],
    )
    .unwrap();
    let recovered = contexts
        .recover_reference(&child.reference().unwrap(), 100)
        .await
        .unwrap();
    assert_eq!(recovered.original_requester(), &actor("caller"));
    let StartRegistration::New(start) = peer
        .register_permitted_attempt(PermittedAttempt {
            id: "peer-effect",
            context: &recovered,
            permits: &permits("worker"),
            effect: &provider(),
            request_digest: recovered.reference().unwrap().request_digest(),
            units: 1,
            clock: &f.clock,
        })
        .await
        .unwrap()
    else {
        panic!("new start");
    };
    peer.settle("peer-effect", &start.token, AttemptStatus::Settled)
        .await
        .unwrap();
    f.coordinator
        .revoke_delegation_grant(
            "retire",
            "service",
            1,
            &issuer(&f.grant),
            &f.coordinator.snapshot().await.unwrap().stamp(),
            "retirement",
            &f.clock,
        )
        .await
        .unwrap();
    assert!(
        peer.register_permitted_attempt(PermittedAttempt {
            id: "after-retirement",
            context: &recovered,
            permits: &permits("worker"),
            effect: &provider(),
            request_digest: recovered.reference().unwrap().request_digest(),
            units: 1,
            clock: &f.clock
        })
        .await
        .is_err()
    );
    let snapshot = peer.snapshot().await.unwrap();
    assert_eq!(snapshot.starts.len(), 1);
    for context in [&f.parent, &child] {
        assert_eq!(
            snapshot.roots[&context.execution_id().to_string()].spent_units,
            1
        );
    }
    let journal = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&("city-domain", "request")).unwrap())
    );
    let root_journal = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&("city-domain", "root")).unwrap())
    );
    for (kind, id) in [
        (
            acteon_governance::context::ROOT_ADMISSION_KIND,
            root_journal,
        ),
        (acteon_governance::COORDINATOR_KIND, "authority".to_string()),
        (
            acteon_governance::context::CONTEXT_KIND,
            f.parent.reference().unwrap().context_id().to_string(),
        ),
        (
            acteon_governance::context::CONTEXT_KIND,
            child.reference().unwrap().context_id().to_string(),
        ),
        (acteon_governance::context::CHILD_ADMISSION_KIND, journal),
    ] {
        first
            .delete(&StateKey::new(
                "city",
                "tenant",
                KeyKind::Custom(kind.into()),
                id.as_str(),
            ))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn current_grant_expansion_cannot_widen_the_original_service_intent() {
    let f = Fixture::new().await;
    let expanded = DelegationGrant {
        revision: 2,
        effects: vec![provider(), delete_effect()],
        ..f.grant.clone()
    };
    publish_grant(&f.coordinator, expanded, &f.clock).await;
    let result = f
        .contexts
        .capture_delegated_child(DelegatedContextAdmission {
            admission_key: "expanded",
            parent: &f.parent,
            parent_permits: &permits("caller"),
            grant: reference("service"),
            binding_digest: &f.grant.binding_digest,
            recipient: admission(&f.coordinator, "worker", vec![delete_effect()]).await,
            recipient_permits: &permits("worker"),
            credential: credential("worker"),
            representation: None,
            intent_effects: vec![delete_effect()],
            onward_grants: vec![],
            limits: limits(),
            clock: &f.clock,
        })
        .await;
    assert!(result.is_err());
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
}

#[tokio::test]
async fn retired_grant_replay_still_requires_current_management_bounds() {
    let f = Fixture::new().await;
    f.coordinator
        .revoke_delegation_grant(
            "retire",
            "service",
            1,
            &issuer(&f.grant),
            &f.coordinator.snapshot().await.unwrap().stamp(),
            "retired",
            &f.clock,
        )
        .await
        .unwrap();
    let mut ceiling = issuer(&f.grant);
    ceiling.sources = vec![actor("other-source")];
    assert!(
        f.coordinator
            .revoke_delegation_grant(
                "retire",
                "service",
                1,
                &ceiling,
                &f.coordinator.snapshot().await.unwrap().stamp(),
                "retired",
                &f.clock
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn root_acceptance_recovers_its_original_grants_without_a_second_budget() {
    use acteon_state::{KeyKind, StateKey};
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let f = Fixture::on(state.clone()).await;
    state
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
            f.parent
                .reference()
                .unwrap()
                .context_id()
                .to_string()
                .as_str(),
        ))
        .await
        .unwrap();
    let recovered = f
        .contexts
        .capture_delegating_root(DelegatingRootAdmission {
            admission_key: "root",
            admission: admission(
                &f.coordinator,
                "caller",
                vec![f.grant.ingress_effect.clone()],
            )
            .await,
            permits: &permits("caller"),
            credential: credential("caller"),
            grants: vec![reference("service")],
            limits: limits(),
            representation: None,
            clock: &f.clock,
        })
        .await
        .unwrap();
    assert_eq!(
        recovered.reference().unwrap(),
        f.parent.reference().unwrap()
    );
    assert_eq!(
        recovered.accepted_delegation_grants(),
        &[reference("service")]
    );
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
}

#[tokio::test]
async fn unsigned_context_edits_cannot_replace_the_original_requester() {
    use acteon_state::{KeyKind, StateKey};
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let f = Fixture::on(state.clone()).await;
    let child = f.child("request").await.unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        child.reference().unwrap().context_id().to_string().as_str(),
    );
    let original = state.get(&key).await.unwrap().unwrap();
    let mut seal: serde_json::Value = serde_json::from_str(&original).unwrap();
    let mut payload: serde_json::Value =
        serde_json::from_str(seal["payload"].as_str().unwrap()).unwrap();
    payload["delegated_from"]["source"]["principal"] =
        serde_json::to_value(actor("pretender")).unwrap();
    seal["payload"] = serde_json::to_string(&payload).unwrap().into();
    state
        .set(&key, &serde_json::to_string(&seal).unwrap(), None)
        .await
        .unwrap();
    assert!(
        f.contexts
            .recover_reference(&child.reference().unwrap(), 100)
            .await
            .is_err()
    );
    state.set(&key, &original, None).await.unwrap();
    assert_eq!(
        f.contexts
            .recover_reference(&child.reference().unwrap(), 100)
            .await
            .unwrap()
            .original_requester(),
        &actor("caller")
    );
}
