use std::sync::Arc;

use acteon_governance::{
    AttemptStatus, AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND, CoordinationError,
    CoordinatorLimits, StartRegistration,
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

// Deliberately pin the epoch to exercise stale evaluation, while using this
// test domain's incarnation. The ABA test below pins the full old stamp.
async fn start(
    c: &AuthorityCoordinator,
    id: &str,
    subject: &str,
    resource: &str,
    digest: &str,
    generation: u64,
) -> Result<StartRegistration, CoordinationError> {
    let mut stamp = c.snapshot().await?.stamp();
    stamp.generation = generation;
    c.register_start(id, subject, resource, digest, &stamp)
        .await
}

async fn wait_for_fault(faults: &FaultStore) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("coordinator did not reach the controlled CAS barrier");
}

async fn pair(
    store: Arc<dyn StateStore>,
    peer_store: Arc<dyn StateStore>,
) -> (AuthorityCoordinator, AuthorityCoordinator) {
    let a = AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
        .await
        .unwrap();
    let b = AuthorityCoordinator::connect(peer_store, "city", "tenant")
        .await
        .unwrap();
    (a, b)
}

async fn contract(store: Arc<dyn StateStore>, peer_store: Arc<dyn StateStore>) {
    let (a, b) = pair(store, peer_store).await;
    let registration = start(&a, "first", "agent", "building", "digest", 1)
        .await
        .unwrap();
    let StartRegistration::New(record) = registration else {
        panic!("first start must be new")
    };
    b.change(
        "close",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "human",
        "maintenance",
    )
    .await
    .unwrap();
    assert!(matches!(
        start(&a, "late", "agent", "building", "digest", 1).await,
        Err(CoordinationError::StaleAuthority)
    ));
    assert!(matches!(
        start(&a, "late", "agent", "building", "digest", 2).await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        start(&b, "first", "agent", "building", "digest", 1)
            .await
            .unwrap(),
        StartRegistration::Existing(record.clone())
    );
    assert!(matches!(
        start(&b, "first", "other", "building", "digest", 1).await,
        Err(CoordinationError::Conflict)
    ));
    a.settle("first", &record.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    assert_eq!(
        b.snapshot().await.unwrap().starts["first"].status,
        AttemptStatus::Uncertain
    );
    b.change(
        "reopen",
        AuthorityChange::ReopenResource {
            resource: "building".into(),
        },
        "human",
        "maintenance complete",
    )
    .await
    .unwrap();
    b.change(
        "revoke",
        AuthorityChange::RevokeSubject {
            subject: "agent".into(),
        },
        "human",
        "compromise",
    )
    .await
    .unwrap();
    // Reopening a resource never restores a revoked subject.
    assert!(matches!(
        start(&a, "revoked", "agent", "building", "digest", 4).await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        start(&a, "other", "service", "building", "digest", 4)
            .await
            .unwrap(),
        StartRegistration::New(_)
    ));
    a.acknowledge_change("close").await.unwrap();
    let snapshot = b.snapshot().await.unwrap();
    assert!(!snapshot.changes["close"].pending);
    assert_eq!(snapshot.changes["close"].actor, "human");
    // Replaying a previously acknowledged close does not re-close the building.
    a.change(
        "close",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "human",
        "maintenance",
    )
    .await
    .unwrap();
    assert!(
        !b.snapshot()
            .await
            .unwrap()
            .closed_resources
            .contains("building")
    );
}

#[tokio::test]
async fn two_instances_share_authority_and_honest_replay_state() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    contract(store.clone(), store).await;
}

#[tokio::test]
async fn close_wins_while_start_is_paused_before_registration_cas() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    close_wins_while_start_is_paused_before_registration_cas_contract(store.clone(), store).await;
}

async fn close_wins_while_start_is_paused_before_registration_cas_contract(
    store: Arc<dyn StateStore>,
    peer_store: Arc<dyn StateStore>,
) {
    let faults = Arc::new(FaultStore::new(store.clone()));
    let (a, b) = pair(faults.clone(), peer_store).await;
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let task =
        tokio::spawn(async move { start(&a, "race", "agent", "building", "digest", 1).await });
    wait_for_fault(&faults).await;
    b.change(
        "close",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "human",
        "maintenance",
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::StaleAuthority)
    ));
    assert!(b.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn start_wins_even_if_registration_ack_follows_close() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    start_wins_even_if_registration_ack_follows_close_contract(store.clone(), store).await;
}

async fn start_wins_even_if_registration_ack_follows_close_contract(
    store: Arc<dyn StateStore>,
    peer_store: Arc<dyn StateStore>,
) {
    let faults = Arc::new(FaultStore::new(store.clone()));
    let (a, b) = pair(faults.clone(), peer_store).await;
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    let task =
        tokio::spawn(async move { start(&a, "race", "agent", "building", "digest", 1).await });
    wait_for_fault(&faults).await;
    b.change(
        "close",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "human",
        "maintenance",
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        StartRegistration::New(_)
    ));
    assert_eq!(
        b.snapshot().await.unwrap().starts["race"]
            .authority
            .generation,
        1
    );
}

#[tokio::test]
async fn response_loss_keeps_restriction_and_pending_outbox_and_never_restarts_effect() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    response_loss_keeps_restriction_and_pending_outbox_and_never_restarts_effect_contract(
        store.clone(),
        store,
    )
    .await;
}

async fn response_loss_keeps_restriction_and_pending_outbox_and_never_restarts_effect_contract(
    store: Arc<dyn StateStore>,
    peer_store: Arc<dyn StateStore>,
) {
    let faults = Arc::new(FaultStore::new(store.clone()));
    let (a, b) = pair(faults.clone(), peer_store).await;
    let kind = KeyKind::Custom(COORDINATOR_KIND.into());
    faults
        .fail_next(
            kind.clone(),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        start(&a, "lost", "agent", "building", "digest", 1)
            .await
            .is_err()
    );
    assert!(matches!(
        start(&b, "lost", "agent", "building", "digest", 1)
            .await
            .unwrap(),
        StartRegistration::Existing(_)
    ));
    faults
        .fail_next(kind, WriteOperation::CompareAndSwap, FaultTiming::After)
        .unwrap();
    assert!(
        a.change(
            "lost-close",
            AuthorityChange::CloseResource {
                resource: "building".into()
            },
            "human",
            "maintenance"
        )
        .await
        .is_err()
    );
    let snapshot = b.snapshot().await.unwrap();
    assert!(snapshot.closed_resources.contains("building"));
    assert!(snapshot.changes["lost-close"].pending);
    assert!(matches!(
        start(
            &b,
            "blocked",
            "agent",
            "building",
            "digest",
            snapshot.generation
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn concurrent_starts_do_not_exceed_capacity_and_uncertainty_retains_it() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let a = AuthorityCoordinator::initialize(
        store,
        "city",
        "tenant",
        CoordinatorLimits {
            max_active: 3,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut tasks = Vec::new();
    for i in 0..24 {
        let a = a.clone();
        tasks.push(tokio::spawn(async move {
            start(&a, &format!("call-{i}"), "agent", "building", "digest", 1).await
        }));
    }
    let mut admitted = 0;
    for task in tasks {
        if task.await.unwrap().is_ok() {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 3);
    let state = a.snapshot().await.unwrap();
    let (id, record) = state.starts.iter().next().unwrap();
    a.settle(id, &record.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    assert!(matches!(
        start(&a, "fourth", "agent", "building", "digest", 1).await,
        Err(CoordinationError::Capacity)
    ));
    a.change(
        "close",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "human",
        "capacity is not permission",
    )
    .await
    .unwrap();
    a.settle(id, &record.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert!(matches!(
        a.settle(id, &record.token, AttemptStatus::Uncertain).await,
        Err(CoordinationError::Conflict)
    ));
}

#[tokio::test]
async fn missing_or_incompatible_authority_fails_closed() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    assert!(
        AuthorityCoordinator::connect(store.clone(), "city", "tenant")
            .await
            .is_err()
    );
    let a = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let mut value = serde_json::to_value(a.snapshot().await.unwrap()).unwrap();
    value["schema_version"] = 99.into();
    store.set(&key, &value.to_string(), None).await.unwrap();
    assert!(
        start(&a, "blocked", "agent", "building", "digest", 1)
            .await
            .is_err()
    );
    store.delete(&key).await.unwrap();
    assert!(a.snapshot().await.is_err());
    assert!(
        AuthorityCoordinator::connect(store, "city", "tenant")
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; run explicitly against a real Redis"]
async fn independent_redis_connections_pass_the_same_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let url = std::env::var("ACTEON_GOVERNANCE_REDIS_URL").expect("Redis URL required");
    let config = RedisConfig {
        url,
        prefix: format!("governance-contract-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let b: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    contract(a.clone(), b.clone()).await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    a.delete(&key).await.unwrap();
    close_wins_while_start_is_paused_before_registration_cas_contract(a.clone(), b.clone()).await;
    a.delete(&key).await.unwrap();
    start_wins_even_if_registration_ack_follows_close_contract(a.clone(), b.clone()).await;
    a.delete(&key).await.unwrap();
    response_loss_keeps_restriction_and_pending_outbox_and_never_restarts_effect_contract(
        a.clone(),
        b,
    )
    .await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    a.delete(&key).await.unwrap();
}

#[tokio::test]
async fn old_incarnation_cannot_register_or_observe_a_recreated_attempt() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (old, _) = pair(store.clone(), store.clone()).await;
    let stamp = old.snapshot().await.unwrap().stamp();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    store.delete(&key).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store.clone(), "city", "tenant")
            .await
            .is_err()
    );
    // Only trusted recovery may bootstrap a new authority domain.
    let (new, _) = pair(store.clone(), store).await;
    let new_stamp = new.snapshot().await.unwrap().stamp();
    assert_ne!(stamp.incarnation, new_stamp.incarnation);
    assert_eq!(stamp.generation, new_stamp.generation);
    assert!(matches!(
        new.register_start("same", "agent", "building", "digest", &stamp)
            .await,
        Err(CoordinationError::StaleAuthority)
    ));
    new.register_start("same", "agent", "building", "digest", &new_stamp)
        .await
        .unwrap();
    assert!(matches!(
        old.register_start("same", "agent", "building", "digest", &stamp)
            .await,
        Err(CoordinationError::StaleAuthority)
    ));
}

#[tokio::test]
async fn settled_history_cannot_consume_reserved_control_records() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = AuthorityCoordinator::initialize(
        store,
        "city",
        "tenant",
        CoordinatorLimits {
            max_active: 1,
            max_records: 20,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for i in 0..4 {
        let id = format!("attempt-{i}");
        let StartRegistration::New(record) = start(&c, &id, "agent", "building", "digest", 1)
            .await
            .unwrap()
        else {
            panic!("new attempt expected")
        };
        c.settle(&id, &record.token, AttemptStatus::Settled)
            .await
            .unwrap();
    }
    assert!(matches!(
        start(&c, "full", "agent", "building", "digest", 1).await,
        Err(CoordinationError::Capacity)
    ));
    c.change(
        "emergency",
        AuthorityChange::CloseResource {
            resource: "building".into(),
        },
        "operator",
        "stop",
    )
    .await
    .unwrap();
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .closed_resources
            .contains("building")
    );
}
