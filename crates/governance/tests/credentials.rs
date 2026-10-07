use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::context::{
    AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
    RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::credential::{CredentialAuthority, CredentialReference};
use acteon_governance::permit::{
    ExecutionPermit, PermitIssuanceCeiling, PermitReference, PermittedAttempt,
};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, CoordinationError, CoordinatorLimits, RootBudgetLimits,
    StartRegistration,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use std::sync::Arc;

fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("agent", PrincipalKind::Agent).unwrap()
}
fn effect(id: &str) -> AcceptedEffect {
    AcceptedEffect {
        operation: "provider.execute".into(),
        resources: vec![ResourceRef::new(ResourceKind::Provider, "city", "tenant", id).unwrap()],
    }
}
fn limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 10,
        max_concurrent: 2,
        deadline_ms: 10_000,
    }
}
fn ceiling() -> PermitIssuanceCeiling {
    PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("issuer", PrincipalKind::Human).unwrap(),
        subjects: vec![actor()],
        effects: vec![effect("read"), effect("write")],
        valid_from_ms: 0,
        limits: limits(),
    }
}
fn policy(id: &str, effects: Vec<AcceptedEffect>) -> ExecutionPermit {
    ExecutionPermit {
        id: id.into(),
        revision: 1,
        subject: actor(),
        effects,
        valid_from_ms: 0,
        limits: limits(),
    }
}
fn credential(id: &str, effects: Vec<AcceptedEffect>) -> CredentialAuthority {
    CredentialAuthority {
        ceiling: policy(id, effects),
        auth_method: "api_key".into(),
        execution_enabled: true,
    }
}
fn refs() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "permit".into(),
        accepted_revision: 1,
    }]
}
fn clock(now: i64) -> ManualClock {
    ManualClock::new(chrono::DateTime::from_timestamp_millis(now).unwrap())
}
async fn publish(
    c: &AuthorityCoordinator,
    p: CredentialAuthority,
    change: &str,
) -> Result<(), CoordinationError> {
    let revision = p.ceiling.revision;
    c.publish_credential(
        change,
        p,
        revision - 1,
        &ceiling(),
        &c.snapshot().await?.stamp(),
        "reviewed",
        100,
    )
    .await
    .map(|_| ())
}
struct Fixture {
    state: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    contexts: TrustedContextStore,
}
async fn fixture(state: Arc<dyn StateStore>) -> Fixture {
    let coordinator = AuthorityCoordinator::initialize(
        state.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    coordinator
        .publish_permit(
            "permit-issue",
            policy("permit", ceiling().effects),
            0,
            &ceiling(),
            &coordinator.snapshot().await.unwrap().stamp(),
            "reviewed",
            100,
        )
        .await
        .unwrap();
    publish(
        &coordinator,
        credential("narrow", vec![effect("read")]),
        "narrow-issue",
    )
    .await
    .unwrap();
    publish(
        &coordinator,
        credential("broad", ceiling().effects),
        "broad-issue",
    )
    .await
    .unwrap();
    let contexts = TrustedContextStore::new(
        state.clone(),
        coordinator.clone(),
        "domain".into(),
        "k".into(),
        vec![ContextSigningKey::new("k".into(), vec![1; 32]).unwrap()],
    )
    .unwrap();
    Fixture {
        state,
        coordinator,
        contexts,
    }
}
async fn capture(
    f: &Fixture,
    id: &str,
    revision: u64,
    effects: Vec<AcceptedEffect>,
) -> Result<VerifiedExecutionContext, acteon_governance::context::ContextError> {
    let admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor(),
            request_digest: "a".repeat(64),
        },
        credential_id: id.into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "placeholder".into(),
        accepted_effects: effects,
        deadline_ms: 10_000,
        evaluated_authority: f.coordinator.snapshot().await.unwrap().stamp(),
    };
    f.contexts
        .capture_credentialed_root(
            admission,
            &refs(),
            CredentialReference {
                id: id.into(),
                accepted_revision: revision,
            },
            limits(),
            &clock(100),
        )
        .await
}
async fn start(
    c: &AuthorityCoordinator,
    context: &VerifiedExecutionContext,
    id: &str,
    e: &AcceptedEffect,
    now: i64,
) -> Result<StartRegistration, CoordinationError> {
    c.register_permitted_attempt(PermittedAttempt {
        id,
        context,
        permits: &refs(),
        effect: e,
        request_digest: &"a".repeat(64),
        units: 1,
        clock: &clock(now),
    })
    .await
}
#[tokio::test]
async fn two_credentials_for_one_actor_cannot_union_their_grants() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    assert!(
        capture(&f, "narrow", 1, vec![effect("write")])
            .await
            .is_err()
    );
    let saved = capture(&f, "narrow", 1, vec![effect("read")])
        .await
        .unwrap();
    assert!(
        start(
            &f.coordinator,
            &saved,
            "forged-write",
            &effect("write"),
            100
        )
        .await
        .is_err()
    );
    assert!(matches!(
        start(&f.coordinator, &saved, "read", &effect("read"), 100)
            .await
            .unwrap(),
        StartRegistration::New(_)
    ));
}
#[tokio::test]
async fn current_narrowing_and_role_disable_deny_future_effects() {
    for disabled in [false, true] {
        let f = fixture(Arc::new(MemoryStateStore::new())).await;
        let saved = capture(&f, "broad", 1, ceiling().effects).await.unwrap();
        start(&f.coordinator, &saved, "before", &effect("write"), 100)
            .await
            .unwrap();
        let mut updated = credential("broad", vec![effect("read")]);
        updated.ceiling.revision = 2;
        updated.execution_enabled = !disabled;
        publish(&f.coordinator, updated, "narrow-update")
            .await
            .unwrap();
        assert!(
            start(&f.coordinator, &saved, "after-write", &effect("write"), 100)
                .await
                .is_err()
        );
        if disabled {
            assert!(
                start(&f.coordinator, &saved, "after-read", &effect("read"), 100)
                    .await
                    .is_err()
            );
        } else {
            assert!(matches!(
                start(&f.coordinator, &saved, "after-read", &effect("read"), 100)
                    .await
                    .unwrap(),
                StartRegistration::New(_)
            ));
        }
    }
}
#[tokio::test]
async fn broadening_cannot_expand_original_credential_ceiling() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let saved = capture(&f, "narrow", 1, vec![effect("read")])
        .await
        .unwrap();
    let mut updated = credential("narrow", ceiling().effects);
    updated.ceiling.revision = 2;
    publish(&f.coordinator, updated, "broaden").await.unwrap();
    assert!(
        start(&f.coordinator, &saved, "write", &effect("write"), 100)
            .await
            .is_err()
    );
    assert!(capture(&f, "narrow", 1, ceiling().effects).await.is_err());
    assert!(capture(&f, "narrow", 2, ceiling().effects).await.is_ok());
}
#[tokio::test]
async fn original_and_current_credential_limits_constrain_root_work() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let saved = capture(&f, "broad", 1, ceiling().effects).await.unwrap();
    start(&f.coordinator, &saved, "one", &effect("read"), 100)
        .await
        .unwrap();
    let mut updated = credential("broad", ceiling().effects);
    updated.ceiling.revision = 2;
    updated.ceiling.limits.max_units = 1;
    publish(&f.coordinator, updated, "shrink-budget")
        .await
        .unwrap();
    assert!(
        start(&f.coordinator, &saved, "two", &effect("read"), 100)
            .await
            .is_err()
    );
    assert_eq!(
        f.coordinator.snapshot().await.unwrap().roots[&saved.execution_id().to_string()]
            .spent_units,
        1
    );
}
#[tokio::test]
async fn revocation_is_terminal_and_other_credentials_remain_independent() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let narrow = capture(&f, "narrow", 1, vec![effect("read")])
        .await
        .unwrap();
    let broad = capture(&f, "broad", 1, ceiling().effects).await.unwrap();
    f.coordinator
        .change(
            "revoke",
            AuthorityChange::RevokeCredential {
                credential_id: "narrow".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    assert!(
        start(
            &f.coordinator,
            &narrow,
            "narrow-after",
            &effect("read"),
            100
        )
        .await
        .is_err()
    );
    assert!(
        start(&f.coordinator, &broad, "broad-after", &effect("read"), 100)
            .await
            .is_ok()
    );
    let mut updated = credential("narrow", vec![effect("read")]);
    updated.ceiling.revision = 2;
    assert!(
        publish(&f.coordinator, updated, "reactivate")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn publication_refuses_retargeting_methods_expansion_and_unbounded_changes() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    for kind in 0..4 {
        let mut updated = credential("narrow", vec![effect("read")]);
        updated.ceiling.revision = 2;
        match kind {
            0 => {
                updated.ceiling.subject =
                    PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap();
            }
            1 => updated.auth_method = "jwt".into(),
            2 => updated.ceiling.limits.max_units = 11,
            _ => updated.ceiling.effects = vec![effect("unreviewed")],
        }
        assert!(
            publish(&f.coordinator, updated, &format!("bad-{kind}"))
                .await
                .is_err()
        );
    }
    assert!(
        f.coordinator
            .change(
                "unbounded",
                AuthorityChange::PublishCredential {
                    credential: credential("rogue", vec![effect("read")])
                },
                "issuer",
                "bad"
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn retained_history_reconstructs_credentials_and_detects_policy_tampering() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut value: serde_json::Value =
        serde_json::from_str(&f.state.get(&key).await.unwrap().unwrap()).unwrap();
    value["credentials"]["narrow"]["authority"]["execution_enabled"] = false.into();
    f.state
        .set(&key, &serde_json::to_string(&value).unwrap(), None)
        .await
        .unwrap();
    assert!(f.coordinator.snapshot().await.is_err());
}
#[tokio::test]
async fn replacement_verifies_the_original_credential_after_key_rotation() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let saved = capture(&f, "narrow", 1, vec![effect("read")])
        .await
        .unwrap();
    let reference = saved.reference().unwrap();
    let other = TrustedContextStore::new(
        f.state.clone(),
        f.coordinator.clone(),
        "domain".into(),
        "new".into(),
        vec![
            ContextSigningKey::new("new".into(), vec![2; 32]).unwrap(),
            ContextSigningKey::new("k".into(), vec![1; 32]).unwrap(),
        ],
    )
    .unwrap();
    let recovered = other.recover_reference(&reference, 100).await.unwrap();
    assert_eq!(
        recovered.credential_authority(),
        Some(&CredentialReference {
            id: "narrow".into(),
            accepted_revision: 1
        })
    );
    assert!(
        start(&f.coordinator, &recovered, "resumed", &effect("read"), 100)
            .await
            .is_ok()
    );
}

async fn controlled_race(
    state: Arc<dyn StateStore>,
    peer_state: Arc<dyn StateStore>,
    after: bool,
    narrow_units: bool,
) {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(state));
    let f = fixture(faults.clone()).await;
    let saved = capture(&f, "broad", 1, ceiling().effects).await.unwrap();
    let reference = saved.reference().unwrap();
    let peer = AuthorityCoordinator::connect(peer_state.clone(), "city", "tenant")
        .await
        .unwrap();
    if narrow_units {
        let mut updated = credential("broad", ceiling().effects);
        updated.ceiling.revision = 2;
        updated.ceiling.limits.max_units = 1;
        publish(&peer, updated, "shrink").await.unwrap();
    }
    let release = faults
        .pause_next(
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            if after {
                FaultTiming::After
            } else {
                FaultTiming::Before
            },
        )
        .unwrap();
    let contender = saved.clone();
    let task = tokio::spawn(async move {
        start(&f.coordinator, &saved, "contender", &effect("read"), 100).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if narrow_units {
        start(&peer, &contender, "winner", &effect("read"), 100)
            .await
            .unwrap();
    } else {
        peer.change(
            "revoke",
            AuthorityChange::RevokeCredential {
                credential_id: "broad".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    }
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    let snapshot = peer.snapshot().await.unwrap();
    if after {
        assert!(matches!(result, Ok(StartRegistration::New(_))));
        assert_eq!(snapshot.starts.len(), 1);
    } else if narrow_units {
        assert!(result.is_err());
        assert_eq!(snapshot.starts.len(), 1);
        assert_eq!(
            snapshot.roots[&contender.execution_id().to_string()].spent_units,
            1
        );
    } else {
        assert!(result.is_err());
        assert!(snapshot.starts.is_empty());
    }
    // All records belong to this isolated fixture/prefix, never live authority.
    peer_state
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
            reference.context_id().to_string(),
        ))
        .await
        .unwrap();
    peer_state
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            "authority",
        ))
        .await
        .unwrap();
}
#[tokio::test]
async fn credential_revocation_and_current_limit_races_serialize_with_effect_starts() {
    for (after, limits) in [(false, false), (true, false), (false, true)] {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        controlled_race(state.clone(), state, after, limits).await;
    }
}
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against real Redis"]
async fn independent_redis_credential_authority_passes_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("credential-contract-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let state: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    for (after, limits) in [(false, false), (true, false), (false, true)] {
        controlled_race(state.clone(), peer.clone(), after, limits).await;
    }
}

#[tokio::test]
async fn lost_publication_ack_is_reconciled_without_duplicate_control_events() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let f = fixture(faults.clone()).await;
        let mut updated = credential("broad", vec![effect("read")]);
        updated.ceiling.revision = 2;
        faults
            .fail_next(
                KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(
            publish(&f.coordinator, updated.clone(), "reload")
                .await
                .is_err()
        );
        let before = f.coordinator.snapshot().await.unwrap();
        assert_eq!(
            before.credentials["broad"].authority.ceiling.revision,
            if matches!(timing, FaultTiming::Before) {
                1
            } else {
                2
            }
        );
        publish(&f.coordinator, updated.clone(), "reload")
            .await
            .unwrap();
        publish(&f.coordinator, updated, "reload").await.unwrap();
        let settled = f.coordinator.snapshot().await.unwrap();
        assert_eq!(settled.credentials["broad"].authority.ceiling.revision, 2);
        assert_eq!(settled.changes.len(), 4);
        assert_eq!(settled.generation, 5);
    }
}

async fn durable_admission(f: &Fixture, deadline_ms: i64) -> RootContextAdmission {
    RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor(),
            request_digest: "a".repeat(64),
        },
        credential_id: "broad".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: String::new(),
        accepted_effects: vec![effect("read")],
        deadline_ms,
        evaluated_authority: f.coordinator.snapshot().await.unwrap().stamp(),
    }
}
async fn durable_capture(
    f: &Fixture,
    key: &str,
    now_ms: i64,
    deadline_ms: i64,
) -> Result<VerifiedExecutionContext, acteon_governance::context::ContextError> {
    f.contexts
        .capture_idempotent_credentialed_root(
            key,
            durable_admission(f, deadline_ms).await,
            &refs(),
            CredentialReference {
                id: "broad".into(),
                accepted_revision: 1,
            },
            RootBudgetLimits {
                deadline_ms,
                ..limits()
            },
            &clock(now_ms),
        )
        .await
}

#[tokio::test]
async fn durable_root_replay_preserves_identity_deadline_and_spending_across_replicas() {
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let first = durable_capture(&f, "operation", 100, 5_000).await.unwrap();
    start(
        &f.coordinator,
        &first,
        "spent-attempt",
        &effect("read"),
        100,
    )
    .await
    .unwrap();
    let peer_coordinator = AuthorityCoordinator::connect(f.state.clone(), "city", "tenant")
        .await
        .unwrap();
    let peer = Fixture {
        state: f.state.clone(),
        coordinator: peer_coordinator.clone(),
        contexts: TrustedContextStore::new(
            f.state.clone(),
            peer_coordinator,
            "domain".into(),
            "k".into(),
            vec![ContextSigningKey::new("k".into(), vec![1; 32]).unwrap()],
        )
        .unwrap(),
    };
    let recovered = durable_capture(&peer, "operation", 200, 12_000)
        .await
        .unwrap();
    assert_eq!(first.reference().unwrap(), recovered.reference().unwrap());
    assert_eq!(recovered.deadline_ms(), 5_000);
    let snapshot = peer.coordinator.snapshot().await.unwrap();
    assert_eq!(snapshot.roots.len(), 1);
    assert_eq!(
        snapshot.roots[&first.execution_id().to_string()].spent_units,
        1
    );
    assert_eq!(
        snapshot.roots[&first.execution_id().to_string()].active_attempts,
        1
    );
    assert!(
        durable_capture(&peer, "operation", 5_000, 8_000)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn interrupted_root_admission_recovers_every_write_boundary_without_reallocation() {
    use acteon_governance::{
        COORDINATOR_KIND,
        context::{CONTEXT_KIND, ROOT_ADMISSION_KIND},
    };
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    for (kind, operation) in [
        (ROOT_ADMISSION_KIND, WriteOperation::CheckAndSet),
        (CONTEXT_KIND, WriteOperation::CheckAndSet),
        (COORDINATOR_KIND, WriteOperation::CompareAndSwap),
    ] {
        for timing in [FaultTiming::Before, FaultTiming::After] {
            let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
            let f = fixture(faults.clone()).await;
            let first = durable_admission(&f, 5_000).await;
            faults
                .fail_next(KeyKind::Custom(kind.into()), operation, timing)
                .unwrap();
            assert!(
                f.contexts
                    .capture_idempotent_credentialed_root(
                        "operation",
                        first.clone(),
                        &refs(),
                        CredentialReference {
                            id: "broad".into(),
                            accepted_revision: 1
                        },
                        RootBudgetLimits {
                            deadline_ms: 5_000,
                            ..limits()
                        },
                        &clock(100)
                    )
                    .await
                    .is_err()
            );
            assert_eq!(faults.consumed(), 1);
            // A control change during interrupted publication must not prevent
            // safe recovery of unchanged broad authority, or revive narrow authority.
            f.coordinator
                .change(
                    "unrelated-revocation",
                    AuthorityChange::RevokeCredential {
                        credential_id: "narrow".into(),
                        expected_revision: 1,
                    },
                    "issuer",
                    "stop narrow credential",
                )
                .await
                .unwrap();
            let recovered = durable_capture(&f, "operation", 200, 6_000).await.unwrap();
            if kind != ROOT_ADMISSION_KIND || timing == FaultTiming::After {
                assert_eq!(recovered.execution_id(), first.binding.execution_id);
                assert_eq!(recovered.deadline_ms(), 5_000);
            }
            assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
            let again = durable_capture(&f, "operation", 300, 7_000).await.unwrap();
            assert_eq!(recovered.reference().unwrap(), again.reference().unwrap());
        }
    }
}

#[tokio::test]
async fn concurrent_root_admission_converges_and_conflicting_input_cannot_reuse_the_key() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let f = Arc::new(fixture(faults.clone()).await);
    let release = faults
        .pause_next(
            KeyKind::Custom(acteon_governance::context::ROOT_ADMISSION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    let first_worker = f.clone();
    let first = tokio::spawn(async move {
        durable_capture(&first_worker, "operation", 100, 5_000)
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second = durable_capture(&f, "operation", 100, 5_000).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(
        first.await.unwrap().reference().unwrap(),
        second.reference().unwrap()
    );
    let mut changed = durable_admission(&f, 6_000).await;
    changed.binding.request_digest = "b".repeat(64);
    assert!(
        f.contexts
            .capture_idempotent_credentialed_root(
                "operation",
                changed,
                &refs(),
                CredentialReference {
                    id: "broad".into(),
                    accepted_revision: 1
                },
                RootBudgetLimits {
                    deadline_ms: 6_000,
                    ..limits()
                },
                &clock(200)
            )
            .await
            .is_err()
    );
    f.coordinator
        .change(
            "revoke-root-credential",
            AuthorityChange::RevokeCredential {
                credential_id: "broad".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    assert!(durable_capture(&f, "operation", 300, 7_000).await.is_err());
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
}

#[tokio::test]
async fn admission_integrity_key_binding_and_retained_signing_keys_are_enforced() {
    use sha2::{Digest, Sha256};
    let f = fixture(Arc::new(MemoryStateStore::new())).await;
    let first = durable_capture(&f, "operation", 100, 5_000).await.unwrap();
    let key_for = |operation: &str| {
        StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::context::ROOT_ADMISSION_KIND.into()),
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&("domain", operation)).unwrap())
            ),
        )
    };
    let original_key = key_for("operation");
    let encoded = f.state.get(&original_key).await.unwrap().unwrap();
    // A correctly signed record cannot be transplanted to a different operation.
    f.state
        .set(&key_for("transplanted"), &encoded, None)
        .await
        .unwrap();
    assert!(
        durable_capture(&f, "transplanted", 200, 6_000)
            .await
            .is_err()
    );
    let peer = Fixture {
        state: f.state.clone(),
        coordinator: f.coordinator.clone(),
        contexts: TrustedContextStore::new(
            f.state.clone(),
            f.coordinator.clone(),
            "domain".into(),
            "new".into(),
            vec![
                ContextSigningKey::new("new".into(), vec![2; 32]).unwrap(),
                ContextSigningKey::new("k".into(), vec![1; 32]).unwrap(),
            ],
        )
        .unwrap(),
    };
    assert_eq!(
        durable_capture(&peer, "operation", 200, 12_000)
            .await
            .unwrap()
            .reference()
            .unwrap(),
        first.reference().unwrap()
    );
    let mut tampered: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    let tag = tampered["tag"][0].as_u64().unwrap();
    tampered["tag"][0] = serde_json::json!((tag + 1) % 256);
    f.state
        .set(&original_key, &tampered.to_string(), None)
        .await
        .unwrap();
    assert!(
        durable_capture(&peer, "operation", 300, 7_000)
            .await
            .is_err()
    );
    assert_eq!(f.coordinator.snapshot().await.unwrap().roots.len(), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Preserve and inspect the complete admitted-work cutover contract.
async fn explicit_cutover_preserves_original_provenance_accounting_and_uncertain_work() {
    for protocol in [7, 8] {
        let f = fixture(Arc::new(MemoryStateStore::new())).await;
        let saved = capture(&f, "broad", 1, vec![effect("read")]).await.unwrap();
        let started = start(&f.coordinator, &saved, "in-doubt", &effect("read"), 100)
            .await
            .unwrap();
        let token = match started {
            StartRegistration::New(record) => record.token,
            StartRegistration::Existing(_) => panic!("new start required"),
        };
        f.coordinator
            .settle(
                "in-doubt",
                &token,
                acteon_governance::AttemptStatus::Uncertain,
            )
            .await
            .unwrap();
        f.coordinator
            .change(
                "close-other",
                AuthorityChange::CloseResource {
                    resource: effect("other").resources[0].clone(),
                },
                "operator",
                "close another road",
            )
            .await
            .unwrap();
        let original = serde_json::to_value(f.coordinator.snapshot().await.unwrap()).unwrap();
        let mut source = original.clone();
        source["schema_version"] = protocol.into();
        source.as_object_mut().unwrap().remove("workforce");
        source.as_object_mut().unwrap().remove("agent_registry");
        source.as_object_mut().unwrap().remove("budget_parents");
        source.as_object_mut().unwrap().remove("agent_registry");
        if protocol == 7 {
            source.as_object_mut().unwrap().remove("purpose");
        }
        let key = StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            "authority",
        );
        f.state.set(&key, &source.to_string(), None).await.unwrap();
        let before = f.state.get_versioned(&key).await.unwrap();
        assert!(
            AuthorityCoordinator::plan_scope_upgrade(
                f.state.clone(),
                "city",
                "tenant",
                acteon_governance::ScopePurpose::AuthenticationControl {
                    source_id: "auth-source".into()
                },
                "operator",
                "wrong scope classification"
            )
            .await
            .is_err()
        );
        assert_eq!(f.state.get_versioned(&key).await.unwrap(), before);
        let plan = AuthorityCoordinator::plan_scope_upgrade(
            f.state.clone(),
            "city",
            "tenant",
            acteon_governance::ScopePurpose::Execution,
            "operator",
            "reviewed cutover",
        )
        .await
        .unwrap();
        assert_eq!(f.state.get_versioned(&key).await.unwrap(), before);
        assert_eq!(plan.report().unsettled_starts, 1);
        assert_eq!(plan.report().from_protocol, protocol);
        assert!(plan.apply("unreviewed").await.is_err());
        assert_eq!(f.state.get_versioned(&key).await.unwrap(), before);
        assert!(plan.apply(&plan.report().review_digest).await.unwrap());
        assert!(!plan.apply(&plan.report().review_digest).await.unwrap());
        let peer = AuthorityCoordinator::connect(f.state.clone(), "city", "tenant")
            .await
            .unwrap();
        let migrated = serde_json::to_value(peer.snapshot().await.unwrap()).unwrap();
        for field in [
            "incarnation",
            "namespace",
            "tenant",
            "limits",
            "closed_resources",
            "revoked_subjects",
            "starts",
            "roots",
            "permits",
            "credentials",
            "credential_configurations",
        ] {
            assert_eq!(migrated[field], original[field], "{field}");
        }
        for (id, event) in original["changes"].as_object().unwrap() {
            assert_eq!(migrated["changes"][id], *event);
        }
        assert_eq!(
            migrated["generation"].as_u64().unwrap(),
            original["generation"].as_u64().unwrap() + 1
        );
        let recovered = f
            .contexts
            .recover_reference(&saved.reference().unwrap(), 200)
            .await
            .unwrap();
        assert_eq!(recovered.reference().unwrap(), saved.reference().unwrap());
        let roots = peer.snapshot().await.unwrap().roots;
        assert_eq!(roots[&saved.execution_id().to_string()].spent_units, 1);
        assert_eq!(roots[&saved.execution_id().to_string()].active_attempts, 1);
        assert!(
            start(&peer, &recovered, "outside-permit", &effect("other"), 200)
                .await
                .is_err()
        );
    }
}
