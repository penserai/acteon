use std::sync::Arc;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND, CoordinatorLimits, RootBudgetLimits,
    ScopePurpose, configuration::CredentialConfiguration, context::AcceptedEffect,
    permit::PermitIssuanceCeiling,
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

async fn coordinator(state: Arc<dyn StateStore>) -> AuthorityCoordinator {
    AuthorityCoordinator::initialize(state, "city", "scope", CoordinatorLimits::default())
        .await
        .unwrap()
}
fn control(source: &str) -> ScopePurpose {
    ScopePurpose::AuthenticationControl {
        source_id: source.into(),
    }
}
fn limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 1,
        max_concurrent: 1,
        deadline_ms: 10_000,
    }
}
fn ceiling() -> PermitIssuanceCeiling {
    PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("publisher", PrincipalKind::System).unwrap(),
        subjects: vec![PrincipalIdentity::new("actor", PrincipalKind::Human).unwrap()],
        effects: vec![AcceptedEffect {
            operation: "publish".into(),
            resources: vec![
                ResourceRef::new(ResourceKind::Provider, "city", "scope", "provider").unwrap(),
            ],
        }],
        valid_from_ms: 0,
        limits: limits(),
    }
}
async fn publish(
    c: &AuthorityCoordinator,
    source: &str,
) -> Result<(), acteon_governance::CoordinationError> {
    c.publish_credential_configuration(
        "publish",
        &CredentialConfiguration {
            source_id: source.into(),
            revision: 1,
            configuration_fingerprint: "a".repeat(64),
            credentials: vec![],
        },
        0,
        &ceiling(),
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        100,
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn scope_reservation_is_permanent_idempotent_and_backend_shared() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    let original = c.snapshot().await.unwrap().stamp();
    let reserved = c.reserve_scope(control("auth-source")).await.unwrap();
    assert_ne!(original, reserved);
    let peer = AuthorityCoordinator::connect(state, "city", "scope")
        .await
        .unwrap();
    assert_eq!(
        peer.reserve_scope(control("auth-source")).await.unwrap(),
        reserved
    );
    assert!(peer.reserve_scope(control("other-source")).await.is_err());
    assert!(peer.reserve_scope(ScopePurpose::Execution).await.is_err());
    assert!(peer.reserve_scope(ScopePurpose::Unclaimed).await.is_err());
    assert_eq!(
        peer.snapshot().await.unwrap().purpose,
        control("auth-source")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One complete control-scope isolation and offboarding contract.
async fn control_scope_rejects_execution_and_other_sources_without_mutation() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    c.reserve_scope(control("auth-source")).await.unwrap();
    let before = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    assert!(publish(&c, "other-source").await.is_err());
    let bounded = ceiling();
    let permit = acteon_governance::permit::ExecutionPermit {
        id: "execution".into(),
        revision: 1,
        subject: bounded.subjects[0].clone(),
        effects: bounded.effects.clone(),
        valid_from_ms: 0,
        limits: limits(),
    };
    assert!(
        c.publish_permit(
            "permit",
            permit.clone(),
            0,
            &bounded,
            &c.snapshot().await.unwrap().stamp(),
            "reviewed",
            100
        )
        .await
        .is_err()
    );
    assert!(
        c.publish_credential(
            "credential",
            acteon_governance::credential::CredentialAuthority {
                ceiling: permit,
                auth_method: "api_key".into(),
                execution_enabled: true,
            },
            0,
            &bounded,
            &c.snapshot().await.unwrap().stamp(),
            "reviewed",
            100
        )
        .await
        .is_err()
    );
    let resource = ceiling().effects[0].resources[0].clone();
    assert!(
        c.register_start(
            "effect",
            "actor",
            &resource,
            &"b".repeat(64),
            &c.snapshot().await.unwrap().stamp()
        )
        .await
        .is_err()
    );
    assert!(
        c.create_root_budget(
            "root",
            "actor",
            limits(),
            &c.snapshot().await.unwrap().stamp(),
            100
        )
        .await
        .is_err()
    );
    assert!(
        c.change(
            "closure",
            AuthorityChange::CloseResource { resource },
            "publisher",
            "close"
        )
        .await
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(c.snapshot().await.unwrap()).unwrap(),
        before
    );
    publish(&c, "auth-source").await.unwrap();
    assert_eq!(
        c.snapshot().await.unwrap().credential_configurations.len(),
        1
    );
    c.change(
        "disable-actor",
        AuthorityChange::RevokeSubject {
            subject: "actor".into(),
        },
        "publisher",
        "offboard",
    )
    .await
    .unwrap();
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .revoked_subjects
            .contains("actor")
    );
}

#[tokio::test]
async fn existing_unclaimed_work_is_never_automatically_adopted() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    publish(&c, "legacy-source").await.unwrap();
    let before = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    assert!(c.reserve_scope(ScopePurpose::Execution).await.is_err());
    assert!(c.reserve_scope(control("legacy-source")).await.is_err());
    assert_eq!(
        serde_json::to_value(c.snapshot().await.unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn conflicting_scope_reservations_serialize_at_the_authority_cas() {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let c = coordinator(faults.clone()).await;
    let first = c.clone();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let loser = tokio::spawn(async move { first.reserve_scope(ScopePurpose::Execution).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let winner = c.reserve_scope(control("auth-source")).await.unwrap();
    release.send(()).unwrap();
    assert!(loser.await.unwrap().is_err());
    assert_eq!(c.snapshot().await.unwrap().stamp(), winner);
}

#[tokio::test]
async fn old_protocols_and_missing_scope_ownership_fail_closed_without_recreation() {
    let key = StateKey::new(
        "city",
        "scope",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    for version in 1..=9 {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let c = coordinator(state.clone()).await;
        let mut value = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
        value["schema_version"] = version.into();
        value.as_object_mut().unwrap().remove("purpose");
        let encoded = value.to_string();
        state.set(&key, &encoded, None).await.unwrap();
        assert!(c.snapshot().await.is_err());
        assert!(
            AuthorityCoordinator::initialize(
                state.clone(),
                "city",
                "scope",
                CoordinatorLimits::default()
            )
            .await
            .is_err()
        );
        assert_eq!(state.get(&key).await.unwrap().unwrap(), encoded);
    }
}

#[tokio::test]
async fn lost_scope_reservation_acknowledgements_recover_the_same_owner() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let c = coordinator(faults.clone()).await;
        faults
            .fail_next(
                KeyKind::Custom(COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(c.reserve_scope(ScopePurpose::Execution).await.is_err());
        assert_eq!(faults.consumed(), 1);
        let recovered = c.reserve_scope(ScopePurpose::Execution).await.unwrap();
        assert_eq!(recovered.generation, 2);
        assert!(c.reserve_scope(control("auth-source")).await.is_err());
    }
}

#[tokio::test]
async fn control_scope_cannot_publish_an_actor_only_execution_context() {
    use acteon_governance::context::{
        ContextBinding, ContextSigningKey, ExecutionContextHandle, RootContextAdmission,
        TrustedContextStore,
    };
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    c.reserve_scope(control("auth-source")).await.unwrap();
    let contexts = TrustedContextStore::new(
        state.clone(),
        c.clone(),
        "domain".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![1; 32]).unwrap()],
    )
    .unwrap();
    let handle = ExecutionContextHandle::new();
    assert!(
        contexts
            .capture_root(
                RootContextAdmission {
                    handle: handle.clone(),
                    binding: ContextBinding {
                        execution_id: uuid::Uuid::new_v4(),
                        principal: ceiling().subjects[0].clone(),
                        request_digest: "a".repeat(64)
                    },
                    credential_id: "credential".into(),
                    auth_method: "api_key".into(),
                    accepted_ceiling_revision: "legacy-profile".into(),
                    accepted_effects: ceiling().effects,
                    deadline_ms: 10_000,
                    evaluated_authority: c.snapshot().await.unwrap().stamp(),
                },
                100
            )
            .await
            .is_err()
    );
    let key = StateKey::new(
        "city",
        "scope",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        serde_json::to_value(handle).unwrap().as_str().unwrap(),
    );
    assert!(state.get(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn ownership_metadata_and_reservation_history_cannot_disagree() {
    let key = StateKey::new(
        "city",
        "scope",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    for mutation in 0..3 {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let c = coordinator(state.clone()).await;
        c.reserve_scope(control("auth-source")).await.unwrap();
        let mut value = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
        match mutation {
            0 => value["purpose"] = serde_json::to_value(ScopePurpose::Execution).unwrap(),
            1 => {
                value["changes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("scope-purpose");
            }
            _ => {
                value["changes"]["scope-purpose"]["change"]["purpose"] =
                    serde_json::to_value(ScopePurpose::Execution).unwrap();
            }
        }
        state.set(&key, &value.to_string(), None).await.unwrap();
        assert!(c.snapshot().await.is_err());
        assert!(c.reserve_scope(ScopePurpose::Execution).await.is_err());
    }
}

#[tokio::test]
async fn cutover_requires_the_exact_review_and_fences_cached_writer_versions() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = coordinator(state.clone()).await;
    publish(&c, "auth-source").await.unwrap();
    let plan = AuthorityCoordinator::plan_scope_upgrade(
        state.clone(),
        "city",
        "scope",
        control("auth-source"),
        "operator",
        "reviewed cutover",
    )
    .await
    .unwrap();
    let key = StateKey::new(
        "city",
        "scope",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let (raw, version) = state.get_versioned(&key).await.unwrap().unwrap();
    // Even an identical rewrite invalidates the observed backend CAS version.
    state
        .compare_and_swap(&key, version, &raw, None)
        .await
        .unwrap();
    assert!(plan.apply(&plan.report().review_digest).await.is_err());
    let fresh = AuthorityCoordinator::plan_scope_upgrade(
        state.clone(),
        "city",
        "scope",
        control("auth-source"),
        "operator",
        "reviewed cutover",
    )
    .await
    .unwrap();
    assert_ne!(fresh.report().review_digest, plan.report().review_digest);
    let cached_version = state.get_versioned(&key).await.unwrap().unwrap().1;
    fresh.apply(&fresh.report().review_digest).await.unwrap();
    assert!(matches!(
        state
            .compare_and_swap(&key, cached_version, &raw, None)
            .await
            .unwrap(),
        acteon_state::CasResult::Conflict { .. }
    ));
    assert_eq!(c.snapshot().await.unwrap().purpose, control("auth-source"));
}

#[tokio::test]
async fn cutover_lost_acknowledgement_is_reconciled_without_a_second_reservation() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let c = coordinator(faults.clone()).await;
        publish(&c, "auth-source").await.unwrap();
        let plan = AuthorityCoordinator::plan_scope_upgrade(
            faults.clone(),
            "city",
            "scope",
            control("auth-source"),
            "operator",
            "reviewed cutover",
        )
        .await
        .unwrap();
        faults
            .fail_next(
                KeyKind::Custom(COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        let result = plan.apply(&plan.report().review_digest).await;
        assert_eq!(result.is_ok(), timing == FaultTiming::After);
        plan.apply(&plan.report().review_digest).await.unwrap();
        let snapshot = c.snapshot().await.unwrap();
        assert_eq!(snapshot.changes.len(), 2);
        assert_eq!(snapshot.purpose, control("auth-source"));
    }
}

#[tokio::test]
async fn malformed_legacy_cutovers_never_rewrite_or_bootstrap_authority() {
    let key = StateKey::new(
        "city",
        "scope",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    for mutation in 0..5 {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let c = coordinator(state.clone()).await;
        publish(&c, "auth-source").await.unwrap();
        let mut value = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
        value["schema_version"] = 7.into();
        value.as_object_mut().unwrap().remove("workforce");
        value.as_object_mut().unwrap().remove("budget_parents");
        value.as_object_mut().unwrap().remove("purpose");
        let mut raw = value.to_string();
        match mutation {
            0 => {
                value["schema_version"] = 6.into();
                raw = value.to_string();
            }
            1 => {
                raw = raw.replacen('{', "{\"schema_version\":7,", 1);
            }
            2 => {
                value["extra"] = true.into();
                raw = value.to_string();
            }
            3 => {
                value["credential_configurations"]["auth-source"]["digest"] = "b".repeat(64).into();
                raw = value.to_string();
            }
            _ => {
                value["namespace"] = "another-city".into();
                raw = value.to_string();
            }
        }
        state.set(&key, &raw, None).await.unwrap();
        assert!(
            AuthorityCoordinator::plan_scope_upgrade(
                state.clone(),
                "city",
                "scope",
                control("auth-source"),
                "operator",
                "reviewed cutover"
            )
            .await
            .is_err()
        );
        assert_eq!(state.get(&key).await.unwrap().unwrap(), raw);
    }
    let empty: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    assert!(
        AuthorityCoordinator::plan_scope_upgrade(
            empty.clone(),
            "city",
            "scope",
            ScopePurpose::Execution,
            "operator",
            "reviewed cutover"
        )
        .await
        .is_err()
    );
    assert!(empty.get(&key).await.unwrap().is_none());
}
