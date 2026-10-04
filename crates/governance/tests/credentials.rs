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
