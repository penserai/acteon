//! Discovery is a read-only preview of two independently admitted ceilings.
use acteon_core::{ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityChange, CoordinationError,
    context::AcceptedEffect,
    delegation::{DelegationDiscoveryRequest, DelegationEligibility},
    permit::PermittedAttempt,
};

#[path = "common/delegation.rs"]
mod fixture;
async fn fixture() -> fixture::Fixture {
    fixture::fixture_with_store(std::sync::Arc::new(
        acteon_state_memory::MemoryStateStore::new(),
    ))
    .await
}
use fixture::{Fixture, actor, capture, effect, permits};

async fn discover(f: &Fixture) -> Result<DelegationEligibility, CoordinationError> {
    f.coordinator
        .discover_delegation_eligibility(DelegationDiscoveryRequest {
            parent: &f.parent,
            parent_permits: &permits("planner"),
            recipient: &f.recipient,
            recipient_permits: &permits("responder"),
            target: &actor("responder"),
            agent_resource: &effect().resources[0],
            effect: &effect(),
            clock: &f.clock,
        })
        .await
}
#[tokio::test]
async fn credentialed_participants_preview_without_creating_children_or_spending() {
    let f = fixture().await;
    let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    let eligible = discover(&f).await.unwrap();
    assert_eq!(eligible.target(), &actor("responder"));
    assert_eq!(eligible.checked_at_ms(), 100);
    assert_eq!(
        eligible.authority(),
        &f.coordinator.snapshot().await.unwrap().stamp()
    );
    let after = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
    assert_eq!(before, after);
}
#[tokio::test]
async fn recipient_identity_and_agent_resource_cannot_be_substituted() {
    let f = fixture().await;
    let wrong = actor("privileged-peer");
    let resource = ResourceRef::new(ResourceKind::Provider, "city", "tenant", "responder").unwrap();
    for (target, resource) in [
        (&wrong, &effect().resources[0]),
        (&actor("responder"), &resource),
    ] {
        assert!(
            f.coordinator
                .discover_delegation_eligibility(DelegationDiscoveryRequest {
                    parent: &f.parent,
                    parent_permits: &permits("planner"),
                    recipient: &f.recipient,
                    recipient_permits: &permits("responder"),
                    target,
                    agent_resource: resource,
                    effect: &effect(),
                    clock: &f.clock,
                })
                .await
                .is_err()
        );
    }
}
#[tokio::test]
async fn actor_only_compatibility_contexts_do_not_qualify_for_delegation_discovery() {
    let mut f = fixture().await;
    f.recipient = capture(&f.coordinator, &f.contexts, "responder", &f.clock, false).await;
    assert!(discover(&f).await.is_err());
    f.recipient = capture(&f.coordinator, &f.contexts, "responder", &f.clock, true).await;
    f.parent = capture(&f.coordinator, &f.contexts, "planner", &f.clock, false).await;
    assert!(discover(&f).await.is_err());
}
#[tokio::test]
async fn either_participant_revocation_and_agent_closure_refuse_without_writes() {
    for change in [
        AuthorityChange::RevokeSubject {
            subject: "planner".into(),
        },
        AuthorityChange::RevokeSubject {
            subject: "responder".into(),
        },
        AuthorityChange::RevokeCredential {
            credential_id: "responder-key".into(),
            expected_revision: 1,
        },
        AuthorityChange::RevokePermit {
            permit_id: "planner-permit".into(),
            expected_revision: 1,
        },
        AuthorityChange::CloseResource {
            resource: effect().resources[0].clone(),
        },
        AuthorityChange::CancelExecution {
            execution_id: "placeholder".into(),
        },
    ] {
        let f = fixture().await;
        let change = if matches!(change, AuthorityChange::CancelExecution { .. }) {
            AuthorityChange::CancelExecution {
                execution_id: f.recipient.execution_id().to_string(),
            }
        } else {
            change
        };
        f.coordinator
            .change("restrict", change, "operator", "test restriction")
            .await
            .unwrap();
        let before = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
        assert!(discover(&f).await.is_err());
        assert_eq!(
            before,
            serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap()
        );
    }
}
#[tokio::test]
async fn full_tuple_and_current_budget_are_checked_for_both_participants() {
    let f = fixture().await;
    let expanded = AcceptedEffect {
        resources: vec![
            effect().resources[0].clone(),
            ResourceRef::new(ResourceKind::Agent, "city", "tenant", "other").unwrap(),
        ],
        ..effect()
    };
    assert!(
        f.coordinator
            .discover_delegation_eligibility(DelegationDiscoveryRequest {
                parent: &f.parent,
                parent_permits: &permits("planner"),
                recipient: &f.recipient,
                recipient_permits: &permits("responder"),
                target: &actor("responder"),
                agent_resource: &effect().resources[0],
                effect: &expanded,
                clock: &f.clock,
            })
            .await
            .is_err()
    );
    for (context, id) in [(&f.parent, "planner"), (&f.recipient, "responder")] {
        f.coordinator
            .register_permitted_attempt(PermittedAttempt {
                id,
                context,
                permits: &permits(id),
                effect: &effect(),
                request_digest: &"a".repeat(64),
                units: 1,
                clock: &f.clock,
            })
            .await
            .unwrap();
        assert!(matches!(
            discover(&f).await,
            Err(CoordinationError::ConcurrencyExhausted | CoordinationError::PermitDenied(_))
        ));
    }
}
#[tokio::test]
async fn discovery_refuses_expiry_and_self_delegation() {
    let f = fixture().await;
    assert!(
        f.coordinator
            .discover_delegation_eligibility(DelegationDiscoveryRequest {
                parent: &f.recipient,
                parent_permits: &permits("responder"),
                recipient: &f.recipient,
                recipient_permits: &permits("responder"),
                target: &actor("responder"),
                agent_resource: &effect().resources[0],
                effect: &effect(),
                clock: &f.clock,
            })
            .await
            .is_err()
    );
    f.clock
        .advance_to(std::time::Duration::from_secs(20))
        .unwrap();
    assert!(discover(&f).await.is_err());
}
