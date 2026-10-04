use std::sync::Arc;

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND, CoordinatorLimits,
    context::{
        AcceptedEffect, CONTEXT_KIND, ContextBinding, ContextError, ContextSigningKey,
        ExecutionContextHandle, RootContextAdmission, TrustedContextStore,
    },
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use uuid::Uuid;

fn actor(id: &str) -> PrincipalIdentity {
    PrincipalIdentity::new(id, PrincipalKind::Agent).unwrap()
}
fn effect(operation: &str, id: &str) -> AcceptedEffect {
    AcceptedEffect {
        operation: operation.into(),
        resources: vec![ResourceRef::new(ResourceKind::Provider, "city", "tenant", id).unwrap()],
    }
}
fn key(id: &str, byte: u8) -> ContextSigningKey {
    ContextSigningKey::new(id.into(), vec![byte; 32]).unwrap()
}
fn context_store(
    store: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
) -> TrustedContextStore {
    TrustedContextStore::new(
        store,
        coordinator,
        "domain".into(),
        "k1".into(),
        vec![key("k1", 1)],
    )
    .unwrap()
}
async fn fixture() -> (
    Arc<dyn StateStore>,
    AuthorityCoordinator,
    TrustedContextStore,
    RootContextAdmission,
) {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: Uuid::new_v4(),
            principal: actor("investigator"),
            request_digest: "a".repeat(64),
        },
        credential_id: "original-key".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "ceiling-1".into(),
        accepted_effects: vec![
            effect("diagnose", "diagnostic"),
            effect("read", "inventory"),
        ],
        deadline_ms: 10_000,
        evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
    };
    let contexts = context_store(store.clone(), coordinator.clone());
    (store, coordinator, contexts, admission)
}
fn record_key(handle: &ExecutionContextHandle) -> StateKey {
    StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(CONTEXT_KIND.into()),
        serde_json::to_value(handle).unwrap().as_str().unwrap(),
    )
}

#[tokio::test]
async fn another_replica_recovers_original_actor_and_exact_ceiling() {
    let (store, _, contexts, admission) = fixture().await;
    let captured = contexts.capture_root(admission.clone(), 100).await.unwrap();
    let peer = AuthorityCoordinator::connect(store.clone(), "city", "tenant")
        .await
        .unwrap();
    let replacement = context_store(store, peer);
    let recovered = replacement
        .recover(captured.handle(), &admission.binding, 200)
        .await
        .unwrap();
    assert_eq!(recovered.principal(), &actor("investigator"));
    assert_eq!(recovered.credential_id(), "original-key");
    assert_eq!(recovered.accepted_ceiling_revision(), "ceiling-1");
    assert!(recovered.within_accepted_ceiling(&effect("diagnose", "diagnostic")));
    assert!(!recovered.within_accepted_ceiling(&effect("diagnose", "inventory")));
    assert!(!recovered.within_accepted_ceiling(&effect("remediate", "diagnostic")));
    let mut duplicate = effect("diagnose", "diagnostic");
    duplicate.resources.push(duplicate.resources[0].clone());
    assert!(!recovered.within_accepted_ceiling(&duplicate));
}

#[tokio::test]
async fn handle_does_not_authenticate_owner_or_input() {
    let (_, _, contexts, admission) = fixture().await;
    contexts.capture_root(admission.clone(), 100).await.unwrap();
    for binding in [
        ContextBinding {
            principal: actor("other"),
            ..admission.binding.clone()
        },
        ContextBinding {
            execution_id: Uuid::new_v4(),
            ..admission.binding.clone()
        },
        ContextBinding {
            request_digest: "b".repeat(64),
            ..admission.binding.clone()
        },
    ] {
        assert!(matches!(
            contexts.recover(&admission.handle, &binding, 200).await,
            Err(ContextError::Verification)
        ));
    }
    assert!(matches!(
        contexts
            .recover(&ExecutionContextHandle::new(), &admission.binding, 200)
            .await,
        Err(ContextError::Missing)
    ));
}

#[tokio::test]
async fn modification_unknown_formats_and_record_transplant_fail_closed() {
    let (store, _, contexts, admission) = fixture().await;
    contexts.capture_root(admission.clone(), 100).await.unwrap();
    let key = record_key(&admission.handle);
    let original = store.get(&key).await.unwrap().unwrap();
    for mutation in ["payload", "schema_version", "key_id", "tag", "extra"] {
        let mut wire: serde_json::Value = serde_json::from_str(&original).unwrap();
        match mutation {
            "payload" => {
                let mut payload: serde_json::Value =
                    serde_json::from_str(wire["payload"].as_str().unwrap()).unwrap();
                payload["principal"]["id"] = "other".into();
                wire["payload"] = serde_json::to_string(&payload).unwrap().into();
            }
            "schema_version" => wire[mutation] = 999.into(),
            "key_id" => wire[mutation] = "missing".into(),
            "tag" => wire[mutation] = serde_json::json!([0]),
            _ => wire[mutation] = true.into(),
        }
        store.set(&key, &wire.to_string(), None).await.unwrap();
        assert!(
            matches!(
                contexts
                    .recover(&admission.handle, &admission.binding, 200)
                    .await,
                Err(ContextError::Verification)
            ),
            "{mutation}"
        );
    }
    let transplanted = ExecutionContextHandle::new();
    store
        .set(&record_key(&transplanted), &original, None)
        .await
        .unwrap();
    assert!(matches!(
        contexts
            .recover(&transplanted, &admission.binding, 200)
            .await,
        Err(ContextError::Verification)
    ));
    let other = TrustedContextStore::new(
        store.clone(),
        AuthorityCoordinator::connect(store.clone(), "city", "tenant")
            .await
            .unwrap(),
        "other-domain".into(),
        "k1".into(),
        vec![key_material()],
    )
    .unwrap();
    store.set(&key, &original, None).await.unwrap();
    // The valid signature/handle still cannot cross administrative domains.
    assert!(matches!(
        other
            .recover(&admission.handle, &admission.binding, 200)
            .await,
        Err(ContextError::Verification)
    ));
}
fn key_material() -> ContextSigningKey {
    key("k1", 1)
}

#[tokio::test]
async fn replay_observes_original_capture_and_cannot_broaden_ceiling() {
    let (_, coordinator, contexts, admission) = fixture().await;
    contexts.capture_root(admission.clone(), 100).await.unwrap();
    coordinator
        .change(
            "close",
            AuthorityChange::CloseResource {
                resource: effect("diagnose", "diagnostic").resources.remove(0),
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
    let replay = contexts.capture_root(admission.clone(), 200).await.unwrap();
    assert_eq!(replay.handle(), &admission.handle);
    let mut broadened = admission;
    broadened
        .accepted_effects
        .push(effect("remediate", "production"));
    assert!(matches!(
        contexts.capture_root(broadened, 200).await,
        Err(ContextError::Conflict)
    ));
    // Recovery/capture observation is not a fresh authority decision.
    assert!(
        !coordinator
            .snapshot()
            .await
            .unwrap()
            .closed_resources
            .is_empty()
    );
    assert!(coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn stale_evaluation_expiry_and_recreated_authority_are_refused() {
    let (store, coordinator, contexts, admission) = fixture().await;
    coordinator
        .change(
            "revoke",
            AuthorityChange::RevokeSubject {
                subject: "investigator".into(),
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
    assert!(matches!(
        contexts.capture_root(admission.clone(), 100).await,
        Err(ContextError::Coordination(_))
    ));
    let mut refreshed = admission;
    refreshed.evaluated_authority = coordinator.snapshot().await.unwrap().stamp();
    contexts.capture_root(refreshed.clone(), 100).await.unwrap();
    assert!(matches!(
        contexts
            .recover(&refreshed.handle, &refreshed.binding, 10_000)
            .await,
        Err(ContextError::Expired)
    ));
    assert!(matches!(
        contexts
            .recover(&refreshed.handle, &refreshed.binding, 99)
            .await,
        Err(ContextError::Verification)
    ));
    let authority_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    store.delete(&authority_key).await.unwrap();
    assert!(matches!(
        contexts
            .recover(&refreshed.handle, &refreshed.binding, 200)
            .await,
        Err(ContextError::Coordination(_))
    ));
    AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
        .await
        .unwrap();
    assert!(matches!(
        contexts
            .recover(&refreshed.handle, &refreshed.binding, 200)
            .await,
        Err(ContextError::Incarnation)
    ));
}

#[tokio::test]
async fn rotation_requires_retained_verification_key() {
    let (store, coordinator, contexts, admission) = fixture().await;
    contexts.capture_root(admission.clone(), 100).await.unwrap();
    let rotated = TrustedContextStore::new(
        store.clone(),
        coordinator.clone(),
        "domain".into(),
        "k2".into(),
        vec![key("k1", 1), key("k2", 2)],
    )
    .unwrap();
    assert_eq!(
        rotated
            .recover(&admission.handle, &admission.binding, 200)
            .await
            .unwrap()
            .credential_id(),
        "original-key"
    );
    let retired = TrustedContextStore::new(
        store,
        coordinator,
        "domain".into(),
        "k2".into(),
        vec![key("k2", 2)],
    )
    .unwrap();
    assert!(matches!(
        retired
            .recover(&admission.handle, &admission.binding, 200)
            .await,
        Err(ContextError::Verification)
    ));
}

#[tokio::test]
async fn foreign_resource_and_invalid_admission_are_never_persisted() {
    let (store, _, contexts, admission) = fixture().await;
    let mut foreign = admission.clone();
    foreign.accepted_effects[0].resources[0] =
        ResourceRef::new(ResourceKind::Provider, "city", "tenant.prod", "diagnostic").unwrap();
    assert!(matches!(
        contexts.capture_root(foreign, 100).await,
        Err(ContextError::Invalid)
    ));
    let mut duplicate = admission.clone();
    let repeated = duplicate.accepted_effects[0].resources[0].clone();
    duplicate.accepted_effects[0].resources.push(repeated);
    assert!(matches!(
        contexts.capture_root(duplicate, 100).await,
        Err(ContextError::Invalid)
    ));
    assert!(
        store
            .get(&record_key(&admission.handle))
            .await
            .unwrap()
            .is_none()
    );
    assert!(ContextSigningKey::new("weak".into(), vec![0; 16]).is_err());
}

#[tokio::test]
async fn lost_capture_acknowledgment_replays_one_original_record() {
    let (store, coordinator, _, admission) = fixture().await;
    let faults = Arc::new(FaultStore::new(store.clone()));
    let contexts = context_store(faults.clone(), coordinator);
    faults
        .fail_next(
            KeyKind::Custom(CONTEXT_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        contexts.capture_root(admission.clone(), 100).await,
        Err(ContextError::State(_))
    ));
    let original = store
        .get(&record_key(&admission.handle))
        .await
        .unwrap()
        .unwrap();
    let replay = contexts.capture_root(admission.clone(), 200).await.unwrap();
    assert_eq!(replay.handle(), &admission.handle);
    assert_eq!(
        store
            .get(&record_key(&admission.handle))
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(faults.consumed(), 1);
}

#[tokio::test]
async fn concurrent_capture_cannot_replace_first_accepted_ceiling() {
    let (store, coordinator, contexts, admission) = fixture().await;
    let peer = context_store(store, coordinator);
    let mut broad = admission.clone();
    broad
        .accepted_effects
        .push(effect("remediate", "production"));
    let (a, b) = tokio::join!(
        contexts.capture_root(admission.clone(), 100),
        peer.capture_root(broad, 100)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        if a.is_err() { a } else { b },
        Err(ContextError::Conflict)
    ));
    let original = contexts
        .recover(&admission.handle, &admission.binding, 200)
        .await
        .unwrap();
    assert!(original.within_accepted_ceiling(&effect("diagnose", "diagnostic")));
}

#[tokio::test]
async fn signed_unknown_payload_and_cross_scope_transplant_are_refused() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let (store, _, contexts, admission) = fixture().await;
    contexts.capture_root(admission.clone(), 100).await.unwrap();
    let storage_key = record_key(&admission.handle);
    let original = store.get(&storage_key).await.unwrap().unwrap();
    for extra in [false, true] {
        let mut envelope: serde_json::Value = serde_json::from_str(&original).unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_str(envelope["payload"].as_str().unwrap()).unwrap();
        if extra {
            payload["future_authority"] = true.into();
        } else {
            payload["schema_version"] = 999.into();
        }
        let payload = payload.to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(&[1; 32]).unwrap();
        mac.update(payload.as_bytes());
        envelope["payload"] = payload.into();
        envelope["tag"] = serde_json::to_value(mac.finalize().into_bytes().to_vec()).unwrap();
        store
            .set(&storage_key, &envelope.to_string(), None)
            .await
            .unwrap();
        assert!(matches!(
            contexts
                .recover(&admission.handle, &admission.binding, 200)
                .await,
            Err(ContextError::Verification)
        ));
    }
    let foreign_coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "other",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let foreign = context_store(store.clone(), foreign_coordinator);
    let foreign_key = StateKey::new(
        "city",
        "other",
        KeyKind::Custom(CONTEXT_KIND.into()),
        storage_key.id,
    );
    store.set(&foreign_key, &original, None).await.unwrap();
    assert!(matches!(
        foreign
            .recover(&admission.handle, &admission.binding, 200)
            .await,
        Err(ContextError::Verification)
    ));
}

#[tokio::test]
async fn bounds_and_keyring_configuration_fail_before_capture() {
    let (store, coordinator, contexts, mut admission) = fixture().await;
    admission.accepted_effects = (0..129)
        .map(|i| effect("diagnose", &format!("provider-{i}")))
        .collect();
    assert!(matches!(
        contexts.capture_root(admission.clone(), 100).await,
        Err(ContextError::Invalid)
    ));
    assert!(
        store
            .get(&record_key(&admission.handle))
            .await
            .unwrap()
            .is_none()
    );
    for (active, keys) in [
        ("k1", vec![]),
        ("missing", vec![key("k1", 1)]),
        ("k1", vec![key("k1", 1), key("k1", 2)]),
    ] {
        assert!(matches!(
            TrustedContextStore::new(
                store.clone(),
                coordinator.clone(),
                "domain".into(),
                active.into(),
                keys
            ),
            Err(ContextError::Invalid)
        ));
    }
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; run explicitly against a real Redis"]
async fn independent_redis_context_capture_and_recovery() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").expect("Redis URL required"),
        prefix: format!("governance-context-contract-{}", Uuid::new_v4()),
        ..Default::default()
    };
    let store: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let other: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let (_, _, _, mut admission) = fixture().await;
    let coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    admission.evaluated_authority = coordinator.snapshot().await.unwrap().stamp();
    let faults = Arc::new(FaultStore::new(store.clone()));
    let contexts = context_store(faults.clone(), coordinator);
    faults
        .fail_next(
            KeyKind::Custom(CONTEXT_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        contexts.capture_root(admission.clone(), 100).await,
        Err(ContextError::State(_))
    ));
    let peer = context_store(
        other.clone(),
        AuthorityCoordinator::connect(other.clone(), "city", "tenant")
            .await
            .unwrap(),
    );
    let recovered = peer
        .recover(&admission.handle, &admission.binding, 200)
        .await
        .unwrap();
    assert_eq!(recovered.credential_id(), "original-key");
    assert_eq!(
        peer.capture_root(admission.clone(), 200)
            .await
            .unwrap()
            .handle(),
        &admission.handle
    );
    let mut wrong = admission.binding.clone();
    wrong.principal = actor("other");
    assert!(matches!(
        peer.recover(&admission.handle, &wrong, 200).await,
        Err(ContextError::Verification)
    ));
    let record = record_key(&admission.handle);
    let mut tampered: serde_json::Value =
        serde_json::from_str(&other.get(&record).await.unwrap().unwrap()).unwrap();
    tampered["payload"] = "{}".into();
    other
        .set(&record, &tampered.to_string(), None)
        .await
        .unwrap();
    assert!(matches!(
        peer.recover(&admission.handle, &admission.binding, 200)
            .await,
        Err(ContextError::Verification)
    ));
    // Delete only this test's keys in its unique prefix.
    other.delete(&record).await.unwrap();
    other
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        ))
        .await
        .unwrap();
}
