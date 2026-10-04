use std::sync::Arc;

use acteon_core::{ResourceKind, ResourceRef};
use acteon_governance::{
    AttemptRequest, AttemptStatus, AuthorityChange, AuthorityCoordinator, COORDINATOR_KIND,
    CoordinationError, CoordinatorLimits, RootBudgetLimits, RootReservation, StartRegistration,
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

fn resources() -> Vec<ResourceRef> {
    vec![
        ResourceRef::new(ResourceKind::Agent, "city", "tenant", "diagnostic").unwrap(),
        ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "diagnostic-http").unwrap(),
    ]
}
async fn pair(
    store: Arc<dyn StateStore>,
    peer: Arc<dyn StateStore>,
) -> (AuthorityCoordinator, AuthorityCoordinator) {
    (
        AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
            .await
            .unwrap(),
        AuthorityCoordinator::connect(peer, "city", "tenant")
            .await
            .unwrap(),
    )
}
async fn root(c: &AuthorityCoordinator, max_units: u64, max_concurrent: u64) {
    c.create_root_budget(
        "root",
        "owner",
        RootBudgetLimits {
            max_units,
            max_concurrent,
            deadline_ms: 1000,
        },
        &c.snapshot().await.unwrap().stamp(),
        10,
    )
    .await
    .unwrap();
}
async fn start(
    c: &AuthorityCoordinator,
    id: &str,
    resources: &[ResourceRef],
    units: u64,
    now: i64,
) -> Result<StartRegistration, CoordinationError> {
    c.register_attempt(AttemptRequest {
        id,
        subject: "child",
        resources,
        request_digest: "input-digest",
        expected_authority: &c.snapshot().await?.stamp(),
        reservation: Some(RootReservation {
            root_id: "root".into(),
            units,
        }),
        now_ms: now,
    })
    .await
}
async fn contract(store: Arc<dyn StateStore>, peer: Arc<dyn StateStore>) {
    let (a, b) = pair(store.clone(), peer).await;
    root(&a, 3, 1).await;
    let refs = resources();
    let StartRegistration::New(first) = start(&a, "one", &refs, 1, 20).await.unwrap() else {
        panic!("new start expected")
    };
    let mut reversed = refs.clone();
    reversed.reverse();
    assert_eq!(
        start(&b, "one", &reversed, 1, 30).await.unwrap(),
        StartRegistration::Existing(first.clone())
    );
    assert!(matches!(
        start(&b, "one", &refs, 2, 30).await,
        Err(CoordinationError::Conflict)
    ));
    assert!(matches!(
        start(&b, "two", &refs, 1, 30).await,
        Err(CoordinationError::ConcurrencyExhausted)
    ));
    a.settle("one", &first.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    assert!(matches!(
        start(&b, "two", &refs, 1, 30).await,
        Err(CoordinationError::ConcurrencyExhausted)
    ));
    b.settle("one", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    a.settle("one", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert_eq!(b.snapshot().await.unwrap().roots["root"].active_attempts, 0);
    let StartRegistration::New(second) = start(&b, "two", &refs, 2, 40).await.unwrap() else {
        panic!("new start expected")
    };
    a.settle("two", &second.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert!(matches!(
        start(&b, "three", &refs, 1, 50).await,
        Err(CoordinationError::BudgetExhausted)
    ));
    assert_eq!(a.snapshot().await.unwrap().roots["root"].spent_units, 3);
    assert_eq!(a.snapshot().await.unwrap().roots["root"].active_attempts, 0);
    // Existing observation cannot replenish units even after deadline.
    assert!(matches!(
        start(&b, "one", &refs, 1, 2000).await,
        Ok(StartRegistration::Existing(_))
    ));
}

async fn controlled_race(
    store: Arc<dyn StateStore>,
    peer: Arc<dyn StateStore>,
    timing: FaultTiming,
    close: bool,
) {
    let faults = Arc::new(FaultStore::new(store));
    let (a, b) = pair(faults.clone(), peer).await;
    root(&a, 1, 2).await;
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            timing,
        )
        .unwrap();
    let paused = a.clone();
    let pending = tokio::spawn(async move { start(&paused, "delayed", &resources(), 1, 20).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("attempt did not reach its controlled CAS boundary");
    if close {
        b.change(
            "close",
            AuthorityChange::CloseResource {
                resource: resources()[1].clone(),
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
    } else {
        assert!(matches!(
            start(&b, "winner", &resources(), 1, 30).await,
            Ok(StartRegistration::New(_))
        ));
    }
    release.send(()).unwrap();
    let delayed = pending.await.unwrap();
    let snapshot = b.snapshot().await.unwrap();
    match (timing, close) {
        (FaultTiming::Before, true) => {
            assert!(matches!(delayed, Err(CoordinationError::StaleAuthority)));
            assert_eq!(snapshot.roots["root"].spent_units, 0);
            assert_eq!(snapshot.roots["root"].active_attempts, 0);
            assert!(snapshot.starts.is_empty());
            assert!(matches!(
                start(&b, "fresh", &resources(), 1, 40).await,
                Err(CoordinationError::Restricted)
            ));
        }
        (FaultTiming::Before, false) => {
            assert!(matches!(delayed, Err(CoordinationError::BudgetExhausted)));
            assert_eq!(snapshot.roots["root"].spent_units, 1);
            assert_eq!(snapshot.starts.len(), 1);
        }
        (FaultTiming::After, true) => {
            assert!(matches!(delayed, Ok(StartRegistration::New(_))));
            assert_eq!(snapshot.roots["root"].spent_units, 1);
            assert_eq!(snapshot.roots["root"].active_attempts, 1);
            assert!(snapshot.closed_resources.contains(&resources()[1]));
        }
        _ => panic!("unsupported test ordering"),
    }
}

#[tokio::test]
async fn controlled_resource_closure_and_final_unit_races_are_atomic() {
    for (timing, close) in [
        (FaultTiming::Before, true),
        (FaultTiming::Before, false),
        (FaultTiming::After, true),
    ] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        controlled_race(store.clone(), store, timing, close).await;
    }
}

#[tokio::test]
async fn independent_instances_share_units_and_uncertain_capacity() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    contract(store.clone(), store).await;
}

#[tokio::test]
async fn concurrent_descendants_cannot_spend_the_last_unit_twice() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (a, b) = pair(store.clone(), store).await;
    root(&a, 1, 2).await;
    let refs = resources();
    let (left, right) = tokio::join!(
        start(&a, "left", &refs, 1, 20),
        start(&b, "right", &refs, 1, 20)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(matches!(
        if left.is_err() { left } else { right },
        Err(CoordinationError::BudgetExhausted)
    ));
    let snapshot = a.snapshot().await.unwrap();
    assert_eq!(snapshot.starts.len(), 1);
    assert_eq!(snapshot.roots["root"].spent_units, 1);
    assert_eq!(snapshot.roots["root"].active_attempts, 1);
}

#[tokio::test]
async fn closing_any_resource_or_revoking_root_owner_prevents_atomic_spend() {
    for revoke in [false, true] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let (a, b) = pair(store.clone(), store).await;
        root(&a, 10, 2).await;
        let refs = resources();
        a.change(
            "stop",
            if revoke {
                AuthorityChange::RevokeSubject {
                    subject: "owner".into(),
                }
            } else {
                AuthorityChange::CloseResource {
                    resource: refs[1].clone(),
                }
            },
            "operator",
            "stop",
        )
        .await
        .unwrap();
        assert!(matches!(
            start(&b, "blocked", &refs, 1, 20).await,
            Err(CoordinationError::Restricted)
        ));
        let snapshot = a.snapshot().await.unwrap();
        assert!(snapshot.starts.is_empty());
        assert_eq!(snapshot.roots["root"].spent_units, 0);
        assert_eq!(snapshot.roots["root"].active_attempts, 0);
    }
}

#[tokio::test]
async fn root_replay_cannot_increase_limits_and_deadline_is_current() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (a, b) = pair(store.clone(), store).await;
    root(&a, 2, 1).await;
    let stamp = a.snapshot().await.unwrap().stamp();
    assert!(matches!(
        b.create_root_budget(
            "root",
            "owner",
            RootBudgetLimits {
                max_units: 3,
                max_concurrent: 1,
                deadline_ms: 1000
            },
            &stamp,
            20
        )
        .await,
        Err(CoordinationError::Conflict)
    ));
    assert!(matches!(
        start(&b, "expired", &resources(), 1, 1000).await,
        Err(CoordinationError::DeadlineExceeded)
    ));
    assert!(a.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn invalid_resource_sets_and_missing_roots_fail_without_spending() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (a, _) = pair(store.clone(), store).await;
    root(&a, 10, 2).await;
    let refs = resources();
    for invalid in [
        vec![],
        vec![refs[0].clone(), refs[0].clone()],
        vec![ResourceRef::new(ResourceKind::Agent, "city", "other", "peer").unwrap()],
    ] {
        assert!(matches!(
            start(&a, "bad", &invalid, 1, 20).await,
            Err(CoordinationError::Invalid(_))
        ));
    }
    assert!(matches!(
        start(&a, "zero", &refs, 0, 20).await,
        Err(CoordinationError::Invalid(_))
    ));
    let stamp = a.snapshot().await.unwrap().stamp();
    assert!(matches!(
        a.register_attempt(AttemptRequest {
            id: "missing",
            subject: "child",
            resources: &refs,
            request_digest: "digest",
            expected_authority: &stamp,
            reservation: Some(RootReservation {
                root_id: "missing".into(),
                units: 1
            }),
            now_ms: 20
        })
        .await,
        Err(CoordinationError::Conflict)
    ));
    assert_eq!(a.snapshot().await.unwrap().roots["root"].spent_units, 0);
}

#[tokio::test]
async fn lost_registration_and_settlement_acknowledgments_do_not_double_account() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(store.clone()));
    let (a, b) = pair(faults.clone(), store).await;
    root(&a, 2, 1).await;
    let refs = resources();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        start(&a, "one", &refs, 1, 20).await,
        Err(CoordinationError::State(_))
    ));
    let StartRegistration::Existing(record) = start(&b, "one", &refs, 1, 30).await.unwrap() else {
        panic!("recovery observes original registration")
    };
    assert_eq!(b.snapshot().await.unwrap().roots["root"].spent_units, 1);
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        a.settle("one", &record.token, AttemptStatus::Settled).await,
        Err(CoordinationError::State(_))
    ));
    b.settle("one", &record.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert_eq!(b.snapshot().await.unwrap().roots["root"].active_attempts, 0);
    assert_eq!(b.snapshot().await.unwrap().roots["root"].spent_units, 1);
}

#[tokio::test]
async fn corrupt_accounting_and_old_formats_fail_closed_on_reconnect() {
    for mutation in ["spent", "active", "format", "resources", "token"] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let (a, _) = pair(store.clone(), store.clone()).await;
        root(&a, 2, 1).await;
        start(&a, "one", &resources(), 1, 20).await.unwrap();
        let key = StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        );
        let mut value = serde_json::to_value(a.snapshot().await.unwrap()).unwrap();
        match mutation {
            "spent" => value["roots"]["root"]["spent_units"] = 0.into(),
            "active" => value["roots"]["root"]["active_attempts"] = 0.into(),
            "resources" => {
                let duplicate = value["starts"]["one"]["resources"][0].clone();
                value["starts"]["one"]["resources"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            "token" => value["starts"]["one"]["token"] = uuid::Uuid::nil().to_string().into(),
            _ => value["schema_version"] = 2.into(),
        }
        store.set(&key, &value.to_string(), None).await.unwrap();
        assert!(
            AuthorityCoordinator::connect(store, "city", "tenant")
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn byte_capacity_failure_does_not_partially_reserve_root_units() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let a = AuthorityCoordinator::initialize(
        store,
        "city",
        "tenant",
        CoordinatorLimits {
            max_active: 5,
            max_records: 100,
            max_bytes: 20_000,
        },
    )
    .await
    .unwrap();
    root(&a, 10, 5).await;
    let large: Vec<_> = (0..16)
        .map(|i| {
            ResourceRef::new(
                ResourceKind::Endpoint,
                "city",
                "tenant",
                format!("{i}-{}", "x".repeat(1000)),
            )
            .unwrap()
        })
        .collect();
    assert!(matches!(
        start(&a, "too-large", &large, 1, 20).await,
        Err(CoordinationError::Capacity)
    ));
    let snapshot = a.snapshot().await.unwrap();
    assert!(snapshot.starts.is_empty());
    assert_eq!(snapshot.roots["root"].spent_units, 0);
    assert_eq!(snapshot.roots["root"].active_attempts, 0);
}

#[tokio::test]
async fn root_creation_loss_preserves_one_immutable_allocation_and_control_headroom() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(store.clone()));
    let a = AuthorityCoordinator::initialize(
        faults.clone(),
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
    let b = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap();
    let stamp = a.snapshot().await.unwrap().stamp();
    let limits = RootBudgetLimits {
        max_units: 1,
        max_concurrent: 1,
        deadline_ms: 1000,
    };
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(matches!(
        a.create_root_budget("root", "owner", limits.clone(), &stamp, 10)
            .await,
        Err(CoordinationError::State(_))
    ));
    b.create_root_budget("root", "owner", limits.clone(), &stamp, 20)
        .await
        .unwrap();
    assert_eq!(b.snapshot().await.unwrap().roots.len(), 1);
    for i in 0..3 {
        b.create_root_budget(&format!("other-{i}"), "owner", limits.clone(), &stamp, 20)
            .await
            .unwrap();
    }
    assert!(matches!(
        b.create_root_budget("full", "owner", limits, &stamp, 20)
            .await,
        Err(CoordinationError::Capacity)
    ));
    b.change(
        "emergency",
        AuthorityChange::CloseResource {
            resource: resources()[1].clone(),
        },
        "operator",
        "stop",
    )
    .await
    .unwrap();
    assert!(
        b.snapshot()
            .await
            .unwrap()
            .closed_resources
            .contains(&resources()[1])
    );
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against a real Redis"]
async fn independent_redis_root_reservations_pass_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").expect("Redis URL required"),
        prefix: format!("root-reservations-{}", uuid::Uuid::new_v4()),
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
    for (timing, close) in [
        (FaultTiming::Before, true),
        (FaultTiming::Before, false),
        (FaultTiming::After, true),
    ] {
        controlled_race(a.clone(), b.clone(), timing, close).await;
        a.delete(&key).await.unwrap();
    }
}
