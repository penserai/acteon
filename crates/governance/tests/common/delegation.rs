//! Discovery is a read-only preview of two independently admitted ceilings.
use std::sync::Arc;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityCoordinator, CoordinatorLimits, RootBudgetLimits,
    context::{
        AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
        RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
    },
    credential::{CredentialAuthority, CredentialReference},
    permit::{ExecutionPermit, PermitIssuanceCeiling, PermitReference},
};
use acteon_state::StateStore;
use acteon_time::ManualClock;

pub(crate) fn actor(id: &str) -> PrincipalIdentity {
    PrincipalIdentity::new(id, PrincipalKind::Agent).unwrap()
}
pub(crate) fn effect() -> AcceptedEffect {
    AcceptedEffect {
        operation: "agent.invoke".into(),
        resources: vec![
            ResourceRef::new(ResourceKind::Agent, "city", "tenant", "responder").unwrap(),
        ],
    }
}
pub(crate) fn limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 2,
        max_concurrent: 1,
        deadline_ms: 10_000,
    }
}
pub(crate) fn permits(id: &str) -> Vec<PermitReference> {
    vec![PermitReference {
        id: format!("{id}-permit"),
        accepted_revision: 1,
    }]
}
pub(crate) struct Fixture {
    pub(crate) coordinator: AuthorityCoordinator,
    pub(crate) contexts: TrustedContextStore,
    pub(crate) clock: ManualClock,
    pub(crate) parent: VerifiedExecutionContext,
    pub(crate) recipient: VerifiedExecutionContext,
}
async fn publish(coordinator: &AuthorityCoordinator, id: &str) {
    let ceiling = PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("operator", PrincipalKind::Human).unwrap(),
        subjects: vec![actor(id)],
        effects: vec![effect()],
        valid_from_ms: 0,
        limits: limits(),
    };
    let permit = ExecutionPermit {
        id: format!("{id}-permit"),
        revision: 1,
        subject: actor(id),
        effects: vec![effect()],
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
            "reviewed",
            100,
        )
        .await
        .unwrap();
    let credential = CredentialAuthority {
        ceiling: ExecutionPermit {
            id: format!("{id}-key"),
            ..permit
        },
        auth_method: "api_key".into(),
        execution_enabled: true,
    };
    coordinator
        .publish_credential(
            &format!("{id}-credential"),
            credential,
            0,
            &ceiling,
            &coordinator.snapshot().await.unwrap().stamp(),
            "reviewed",
            100,
        )
        .await
        .unwrap();
}
pub(crate) async fn capture(
    coordinator: &AuthorityCoordinator,
    contexts: &TrustedContextStore,
    id: &str,
    clock: &ManualClock,
    credentialed: bool,
) -> VerifiedExecutionContext {
    let admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor(id),
            request_digest: "a".repeat(64),
        },
        credential_id: format!("{id}-key"),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "placeholder".into(),
        accepted_effects: vec![effect()],
        deadline_ms: limits().deadline_ms,
        evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
    };
    if credentialed {
        contexts
            .capture_credentialed_root(
                admission,
                &permits(id),
                CredentialReference {
                    id: format!("{id}-key"),
                    accepted_revision: 1,
                },
                limits(),
                clock,
            )
            .await
            .unwrap()
    } else {
        contexts
            .capture_permitted_root(admission, &permits(id), limits(), clock)
            .await
            .unwrap()
    }
}
pub(crate) async fn fixture_with_store(store: Arc<dyn StateStore>) -> Fixture {
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
    publish(&coordinator, "planner").await;
    publish(&coordinator, "responder").await;
    let contexts = TrustedContextStore::new(
        store,
        coordinator.clone(),
        "city-domain".into(),
        "k".into(),
        vec![ContextSigningKey::new("k".into(), vec![1; 32]).unwrap()],
    )
    .unwrap();
    let clock = ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap());
    let parent = capture(&coordinator, &contexts, "planner", &clock, true).await;
    let recipient = capture(&coordinator, &contexts, "responder", &clock, true).await;
    Fixture {
        coordinator,
        contexts,
        clock,
        parent,
        recipient,
    }
}
