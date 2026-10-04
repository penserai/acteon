use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::configuration::CredentialConfiguration;
use acteon_governance::context::{
    AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
    RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::credential::{CredentialAuthority, CredentialReference};
use acteon_governance::permit::{
    ExecutionPermit, PermitIssuanceCeiling, PermitReference, PermittedAttempt,
};
use acteon_governance::{
    AuthorityCoordinator, COORDINATOR_KIND, CoordinatorLimits, RootBudgetLimits, StartRegistration,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
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
fn policy(id: &str, revision: u64) -> ExecutionPermit {
    ExecutionPermit {
        id: id.into(),
        revision,
        subject: actor(),
        effects: ceiling().effects,
        valid_from_ms: 0,
        limits: limits(),
    }
}
fn configuration(revision: u64) -> CredentialConfiguration {
    CredentialConfiguration {
        source_id: "auth".into(),
        revision,
        configuration_fingerprint: format!("{revision:064x}"),
        credentials: ["a", "b"]
            .into_iter()
            .map(|id| CredentialAuthority {
                ceiling: policy(id, revision),
                auth_method: "api_key".into(),
                execution_enabled: true,
            })
            .collect(),
    }
}
async fn publish(
    c: &AuthorityCoordinator,
    config: &CredentialConfiguration,
    expected: u64,
    id: &str,
) -> Result<acteon_governance::ChangeRecord, acteon_governance::CoordinationError> {
    c.publish_credential_configuration(
        id,
        config,
        expected,
        &ceiling(),
        &c.snapshot().await?.stamp(),
        "reviewed",
        100,
    )
    .await
}
async fn fixture(state: Arc<dyn StateStore>) -> AuthorityCoordinator {
    let c = AuthorityCoordinator::initialize(state, "city", "tenant", CoordinatorLimits::default())
        .await
        .unwrap();
    c.publish_permit(
        "permit",
        policy("permit", 1),
        0,
        &ceiling(),
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        100,
    )
    .await
    .unwrap();
    publish(&c, &configuration(1), 0, "initial").await.unwrap();
    c
}
async fn context(
    state: Arc<dyn StateStore>,
    c: &AuthorityCoordinator,
    id: &str,
) -> VerifiedExecutionContext {
    let contexts = TrustedContextStore::new(
        state,
        c.clone(),
        "domain".into(),
        "k".into(),
        vec![ContextSigningKey::new("k".into(), vec![1; 32]).unwrap()],
    )
    .unwrap();
    contexts
        .capture_credentialed_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: "a".repeat(64),
                },
                credential_id: id.into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: "placeholder".into(),
                accepted_effects: ceiling().effects,
                deadline_ms: 10_000,
                evaluated_authority: c.snapshot().await.unwrap().stamp(),
            },
            &[PermitReference {
                id: "permit".into(),
                accepted_revision: 1,
            }],
            CredentialReference {
                id: id.into(),
                accepted_revision: 1,
            },
            limits(),
            &acteon_time::ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap()),
        )
        .await
        .unwrap()
}
async fn start(
    c: &AuthorityCoordinator,
    ctx: &VerifiedExecutionContext,
    id: &str,
    op: &str,
) -> Result<StartRegistration, acteon_governance::CoordinationError> {
    c.register_permitted_attempt(PermittedAttempt {
        id,
        context: ctx,
        permits: &[PermitReference {
            id: "permit".into(),
            accepted_revision: 1,
        }],
        effect: &effect(op),
        request_digest: &"a".repeat(64),
        units: 1,
        clock: &acteon_time::ManualClock::new(
            chrono::DateTime::from_timestamp_millis(100).unwrap(),
        ),
    })
    .await
}
#[tokio::test]
async fn snapshot_narrows_and_retires_credentials_in_one_generation() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(state.clone()).await;
    let a = context(state.clone(), &c, "a").await;
    let b = context(state, &c, "b").await;
    let old = c.snapshot().await.unwrap().generation;
    let mut next = configuration(2);
    next.credentials.truncate(1);
    next.credentials[0].ceiling.effects = vec![effect("read")];
    publish(&c, &next, 1, "reload").await.unwrap();
    let current = c.snapshot().await.unwrap();
    assert_eq!(current.generation, old + 1);
    assert_eq!(current.credentials["a"].authority.ceiling.revision, 2);
    assert!(current.credentials["b"].revoked);
    assert!(start(&c, &a, "a-write", "write").await.is_err());
    assert!(start(&c, &b, "b-read", "read").await.is_err());
    assert!(matches!(
        start(&c, &a, "a-read", "read").await.unwrap(),
        StartRegistration::New(_)
    ));
}
#[tokio::test]
async fn stale_replicas_and_same_revision_conflicts_cannot_restore_old_grants() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let old_ref = configuration(1)
        .reference(&c.snapshot().await.unwrap().stamp())
        .unwrap();
    publish(&c, &configuration(2), 1, "reload").await.unwrap();
    assert!(publish(&c, &configuration(1), 0, "initial").await.is_err());
    assert!(c.verify_credential_configuration(&old_ref).await.is_err());
    let mut conflict = configuration(2);
    conflict.configuration_fingerprint = "f".repeat(64);
    assert!(publish(&c, &conflict, 1, "conflict").await.is_err());
    let observed = publish(&c, &configuration(2), 0, "another-replica")
        .await
        .unwrap();
    assert_eq!(observed.generation, c.snapshot().await.unwrap().generation);
    assert_eq!(c.snapshot().await.unwrap().changes.len(), 3);
}
#[tokio::test]
async fn canonical_order_and_skipped_source_revisions_have_stable_binding() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut reordered = configuration(1);
    reordered.credentials.reverse();
    for item in &mut reordered.credentials {
        item.ceiling.effects.reverse();
    }
    let stamp = c.snapshot().await.unwrap().stamp();
    assert_eq!(
        reordered.reference(&stamp).unwrap(),
        configuration(1).reference(&stamp).unwrap()
    );
    publish(&c, &reordered, 0, "replica").await.unwrap();
    publish(&c, &configuration(10), 1, "newer").await.unwrap();
    assert_eq!(
        c.snapshot().await.unwrap().credentials["a"]
            .authority
            .ceiling
            .revision,
        10
    );
    assert!(
        c.verify_credential_configuration(&configuration(10).reference(&stamp).unwrap())
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn owned_ids_cannot_be_adopted_overwritten_or_reactivated() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut other = configuration(2);
    other.source_id = "other".into();
    assert!(publish(&c, &other, 0, "adopt").await.is_err());
    let mut single = configuration(2).credentials.remove(0);
    single.ceiling.revision = 2;
    assert!(
        c.publish_credential(
            "overwrite",
            single,
            1,
            &ceiling(),
            &c.snapshot().await.unwrap().stamp(),
            "reviewed",
            100
        )
        .await
        .is_err()
    );
    let mut empty = configuration(2);
    empty.credentials.clear();
    publish(&c, &empty, 1, "retire-all").await.unwrap();
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .credentials
            .values()
            .all(|r| r.revoked)
    );
    assert!(
        publish(&c, &configuration(3), 2, "reactivate")
            .await
            .is_err()
    );
}
#[tokio::test]
async fn failed_entry_or_retirement_ceiling_leaves_the_complete_snapshot_unchanged() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let before = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    let mut next = configuration(2);
    next.credentials[1].ceiling.effects = vec![effect("unapproved")];
    assert!(publish(&c, &next, 1, "invalid").await.is_err());
    let mut empty = configuration(2);
    empty.credentials.clear();
    let mut narrow = ceiling();
    narrow.effects = vec![effect("read")];
    assert!(
        c.publish_credential_configuration(
            "bad-retirement",
            &empty,
            1,
            &narrow,
            &c.snapshot().await.unwrap().stamp(),
            "reviewed",
            100
        )
        .await
        .is_err()
    );
    assert_eq!(
        serde_json::to_value(c.snapshot().await.unwrap()).unwrap(),
        before
    );
}
#[tokio::test]
async fn configuration_acknowledgment_loss_is_observed_without_duplicate_events() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
        let c = fixture(faults.clone()).await;
        faults
            .fail_next(
                KeyKind::Custom(COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(publish(&c, &configuration(2), 1, "reload").await.is_err());
        publish(&c, &configuration(2), 1, "reload").await.unwrap();
        publish(&c, &configuration(2), 1, "retry").await.unwrap();
        assert_eq!(c.snapshot().await.unwrap().changes.len(), 3);
    }
}
#[tokio::test]
async fn snapshot_history_and_incarnation_references_detect_corruption_or_replacement() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(state.clone()).await;
    let mut reference = configuration(1)
        .reference(&c.snapshot().await.unwrap().stamp())
        .unwrap();
    reference.incarnation = uuid::Uuid::new_v4().to_string();
    assert!(c.verify_credential_configuration(&reference).await.is_err());
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let mut value: serde_json::Value =
        serde_json::from_str(&state.get(&key).await.unwrap().unwrap()).unwrap();
    value["credential_configurations"]["auth"]["digest"] = "f".repeat(64).into();
    state.set(&key, &value.to_string(), None).await.unwrap();
    assert!(c.snapshot().await.is_err());
}
#[tokio::test]
async fn competing_replicas_cannot_publish_conflicting_same_revision_snapshots() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let stamp = c.snapshot().await.unwrap().stamp();
    let a = configuration(2);
    let mut b = configuration(2);
    b.configuration_fingerprint = "f".repeat(64);
    let issuance = ceiling();
    let (first, second) = tokio::join!(
        c.publish_credential_configuration("a", &a, 1, &issuance, &stamp, "reviewed", 100),
        c.publish_credential_configuration("b", &b, 1, &issuance, &stamp, "reviewed", 100)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(c.snapshot().await.unwrap().changes.len(), 3);
}

async fn controlled_snapshot_race(
    state: Arc<dyn StateStore>,
    peer_state: Arc<dyn StateStore>,
    after: bool,
) {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(state));
    let c = fixture(faults.clone()).await;
    let ctx = context(faults.clone(), &c, "a").await;
    let second = context(faults.clone(), &c, "b").await;
    let peer = AuthorityCoordinator::connect(peer_state.clone(), "city", "tenant")
        .await
        .unwrap();
    let context_ids = [
        ctx.reference().unwrap().context_id(),
        second.reference().unwrap().context_id(),
    ];
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            if after {
                FaultTiming::After
            } else {
                FaultTiming::Before
            },
        )
        .unwrap();
    let contender = ctx.clone();
    let task = tokio::spawn(async move { start(&c, &ctx, "contender", "write").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut next = configuration(2);
    next.credentials.truncate(1);
    next.credentials[0].ceiling.effects = vec![effect("read")];
    publish(&peer, &next, 1, "snapshot-reload").await.unwrap();
    let current = peer.snapshot().await.unwrap();
    assert_eq!(
        current.credentials["a"].authority.ceiling.effects,
        vec![effect("read")]
    );
    assert!(current.credentials["b"].revoked);
    assert!(start(&peer, &contender, "after-a", "write").await.is_err());
    assert!(start(&peer, &second, "after-b", "write").await.is_err());
    release.send(()).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    if after {
        assert!(matches!(result, Ok(StartRegistration::New(_))));
        assert_eq!(peer.snapshot().await.unwrap().starts.len(), 1);
    } else {
        assert!(result.is_err());
        assert!(peer.snapshot().await.unwrap().starts.is_empty());
    }
    // Isolated fixture/prefix only, never live authority records.
    for id in context_ids {
        peer_state
            .delete(&StateKey::new(
                "city",
                "tenant",
                KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
                id.to_string(),
            ))
            .await
            .unwrap();
    }
    peer_state
        .delete(&StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        ))
        .await
        .unwrap();
}
#[tokio::test]
async fn configuration_and_effect_start_races_preserve_the_atomic_scope_snapshot() {
    for after in [false, true] {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        controlled_snapshot_race(state.clone(), state, after).await;
    }
}
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against real Redis"]
async fn independent_redis_configuration_snapshots_pass_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("auth-snapshot-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let b: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    for after in [false, true] {
        controlled_snapshot_race(a.clone(), b.clone(), after).await;
    }
}

#[tokio::test]
async fn disabled_credentials_can_have_an_empty_execution_ceiling() {
    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(state.clone()).await;
    let saved = context(state, &c, "a").await;
    let mut next = configuration(2);
    next.credentials[0].execution_enabled = false;
    next.credentials[0].ceiling.effects.clear();
    publish(&c, &next, 1, "deny-all").await.unwrap();
    assert!(start(&c, &saved, "denied", "read").await.is_err());
    let mut invalid = configuration(3);
    invalid.credentials[0].ceiling.effects.clear();
    assert!(publish(&c, &invalid, 2, "empty-enabled").await.is_err());
}
