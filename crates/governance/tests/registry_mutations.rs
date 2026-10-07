//! Registry metadata writes are serialized control effects, not resendable projections.
use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AttemptStatus, AuthorityChange, AuthorityCoordinator, CoordinationError, CoordinatorLimits,
    ScopePurpose,
    control::{ControlChangeAuthorization, ControlChangeCeiling},
    registry::{
        AgentRegistryIssuanceCeiling, AgentRegistryQualification, RegistryProjectionKind,
        registry_projection_digest,
    },
};
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;
use std::{collections::BTreeMap, sync::Arc};

fn actor(id: &str) -> PrincipalIdentity {
    PrincipalIdentity::new(id, PrincipalKind::Human).unwrap()
}
fn agent() -> ResourceRef {
    ResourceRef::new(ResourceKind::Agent, "city", "tenant", "worker").unwrap()
}
fn key() -> StateKey {
    StateKey::new("city", "tenant", KeyKind::BusAgentCard, "worker")
}
fn clock() -> ManualClock {
    ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
}
fn ceiling() -> ControlChangeCeiling {
    ControlChangeCeiling {
        actor: actor("operator"),
        subjects: vec![],
        resources: vec![agent()],
        valid_from_ms: 0,
        deadline_ms: 1000,
    }
}
async fn fixture(store: Arc<dyn StateStore>) -> AuthorityCoordinator {
    let c = AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
        .await
        .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    c
}
async fn qualify(c: &AuthorityCoordinator, revision: u64) -> Result<(), CoordinationError> {
    let q = AgentRegistryQualification {
        agent: agent(),
        target: PrincipalIdentity::new("worker", PrincipalKind::Agent).unwrap(),
        revision,
        bindings: BTreeMap::from([("work".into(), format!("{revision:064x}"))]),
    };
    let bounds = AgentRegistryIssuanceCeiling {
        issuer: actor("operator"),
        approved: vec![q.clone()],
        valid_from_ms: 0,
        deadline_ms: 1000,
    };
    c.publish_agent_registry(
        &format!("qualify-{revision}"),
        q,
        revision - 1,
        &bounds,
        &c.snapshot().await?.stamp(),
        "reviewed",
        &clock(),
    )
    .await?;
    Ok(())
}
async fn begin(
    c: &AuthorityCoordinator,
    id: &str,
    revision: u64,
    version: Option<u64>,
    value: Option<&str>,
) -> Result<(), CoordinationError> {
    c.change_evaluated(
        id,
        AuthorityChange::BeginAgentRegistryMutation {
            agent: agent(),
            expected_revision: revision,
            projection: RegistryProjectionKind::Card,
            expected_projection_version: version,
            input_digest: registry_projection_digest(RegistryProjectionKind::Card, version, value),
        },
        "registry change",
        ControlChangeAuthorization {
            ceiling: &ceiling(),
            evaluated_authority: &c.snapshot().await?.stamp(),
            clock: &clock(),
        },
    )
    .await?;
    Ok(())
}
async fn execute(
    c: &AuthorityCoordinator,
    id: &str,
    value: Option<&str>,
) -> Result<(), CoordinationError> {
    c.execute_agent_registry_mutation(
        id,
        value,
        ControlChangeAuthorization {
            ceiling: &ceiling(),
            evaluated_authority: &c.snapshot().await?.stamp(),
            clock: &clock(),
        },
    )
    .await
}

#[tokio::test]
async fn mutation_of_an_unqualified_agent_fences_publication_and_replay_never_rewrites() {
    let store = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    begin(&c, "edit", 0, None, Some("approved metadata"))
        .await
        .unwrap();
    assert!(qualify(&c, 1).await.is_err());
    assert!(matches!(
        c.acknowledge_change("edit").await,
        Err(CoordinationError::Restricted)
    ));
    execute(&c, "edit", Some("approved metadata"))
        .await
        .unwrap();
    let first = store.get_versioned(&key()).await.unwrap().unwrap();
    qualify(&c, 1).await.unwrap();
    execute(&c, "edit", Some("approved metadata"))
        .await
        .unwrap();
    assert_eq!(first, store.get_versioned(&key()).await.unwrap().unwrap());
    let state = c.snapshot().await.unwrap();
    assert_eq!(state.starts.len(), 1);
    assert!(!state.changes["edit"].pending);
}

#[tokio::test]
async fn mutation_requires_exact_private_control_bounds_and_blocks_competing_intents() {
    let store = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    qualify(&c, 1).await.unwrap();
    begin(&c, "edit", 1, None, Some("new card")).await.unwrap();
    assert!(c.snapshot().await.unwrap().agent_registry["worker"].retired);
    assert!(matches!(
        begin(&c, "other", 1, None, Some("other card")).await,
        Err(CoordinationError::Conflict)
    ));
    assert!(qualify(&c, 2).await.is_err());
    assert!(matches!(
        execute(&c, "edit", Some("substituted")).await,
        Err(CoordinationError::Conflict)
    ));
    for other_actor in [false, true] {
        let mut bounds = ceiling();
        if other_actor {
            bounds.actor = actor("another operator");
        } else {
            bounds.resources.clear();
        }
        assert!(
            c.execute_agent_registry_mutation(
                "edit",
                Some("new card"),
                ControlChangeAuthorization {
                    ceiling: &bounds,
                    evaluated_authority: &c.snapshot().await.unwrap().stamp(),
                    clock: &clock()
                }
            )
            .await
            .is_err()
        );
    }
    assert!(c.snapshot().await.unwrap().starts.is_empty());
    assert!(store.get(&key()).await.unwrap().is_none());
    execute(&c, "edit", Some("new card")).await.unwrap();
    assert!(c.snapshot().await.unwrap().agent_registry["worker"].retired);
    qualify(&c, 2).await.unwrap();
}

#[tokio::test]
async fn lost_projection_acknowledgement_retains_uncertainty_even_when_bytes_match() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let memory = Arc::new(MemoryStateStore::new());
        let faults = Arc::new(FaultStore::new(memory.clone()));
        let c = fixture(faults.clone()).await;
        memory.set(&key(), "old", None).await.unwrap();
        qualify(&c, 1).await.unwrap();
        begin(&c, "edit", 1, Some(1), Some("new")).await.unwrap();
        faults
            .fail_next(
                KeyKind::BusAgentCard,
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(matches!(
            execute(&c, "edit", Some("new")).await,
            Err(CoordinationError::State(_))
        ));
        let retained = memory.get_versioned(&key()).await.unwrap().unwrap();
        assert_eq!(
            retained.0,
            if matches!(timing, FaultTiming::After) {
                "new"
            } else {
                "old"
            }
        );
        assert!(matches!(
            execute(&c, "edit", Some("new")).await,
            Err(CoordinationError::Restricted)
        ));
        assert_eq!(
            retained,
            memory.get_versioned(&key()).await.unwrap().unwrap()
        );
        let state = c.snapshot().await.unwrap();
        assert!(state.changes["edit"].pending);
        assert!(state.agent_registry["worker"].retired);
        assert_eq!(
            state.starts.values().next().unwrap().status,
            AttemptStatus::Uncertain
        );
        assert!(qualify(&c, 2).await.is_err());
        assert!(c.acknowledge_change("edit").await.is_err());
    }
}

#[tokio::test]
async fn lost_completion_acknowledgement_recovers_from_known_receipt_without_resend() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let memory = Arc::new(MemoryStateStore::new());
        let faults = Arc::new(FaultStore::new(memory.clone()));
        let c = fixture(faults.clone()).await;
        begin(&c, "edit", 0, None, Some("new")).await.unwrap();
        faults
            .fail_after_matches(
                KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
                2,
            )
            .unwrap();
        assert!(execute(&c, "edit", Some("new")).await.is_err());
        let known = memory.get_versioned(&key()).await.unwrap().unwrap();
        assert_eq!(
            c.snapshot()
                .await
                .unwrap()
                .starts
                .values()
                .next()
                .unwrap()
                .status,
            AttemptStatus::Settled
        );
        execute(&c, "edit", Some("new")).await.unwrap();
        assert_eq!(known, memory.get_versioned(&key()).await.unwrap().unwrap());
        assert!(!c.snapshot().await.unwrap().changes["edit"].pending);
        qualify(&c, 1).await.unwrap();
    }
}

#[tokio::test]
async fn projection_version_conflict_is_certified_no_effect_and_does_not_overwrite() {
    let store = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    store.set(&key(), "old", None).await.unwrap();
    qualify(&c, 1).await.unwrap();
    begin(&c, "edit", 1, Some(1), Some("new")).await.unwrap();
    store.set(&key(), "concurrent change", None).await.unwrap();
    let observed = store.get_versioned(&key()).await.unwrap().unwrap();
    for _ in 0..2 {
        assert!(matches!(
            execute(&c, "edit", Some("new")).await,
            Err(CoordinationError::Conflict)
        ));
    }
    assert_eq!(
        observed,
        store.get_versioned(&key()).await.unwrap().unwrap()
    );
    let state = c.snapshot().await.unwrap();
    assert!(!state.changes["edit"].pending);
    assert!(state.agent_registry["worker"].retired);
    qualify(&c, 2).await.unwrap();
}

#[tokio::test]
async fn paused_delivery_blocks_requalification_and_concurrent_worker_never_resends() {
    let memory: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    paused_delivery_contract(memory.clone(), memory).await;
}

async fn paused_delivery_contract(memory: Arc<dyn StateStore>, peer_store: Arc<dyn StateStore>) {
    let faults = Arc::new(FaultStore::new(memory.clone()));
    let c = fixture(faults.clone()).await;
    let peer = AuthorityCoordinator::connect(peer_store, "city", "tenant")
        .await
        .unwrap();
    memory.set(&key(), "old", None).await.unwrap();
    qualify(&c, 1).await.unwrap();
    begin(&c, "edit", 1, Some(1), Some("new")).await.unwrap();
    let release = faults
        .pause_next(
            KeyKind::BusAgentCard,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let worker = c.clone();
    let task = tokio::spawn(async move { execute(&worker, "edit", Some("new")).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(qualify(&peer, 2).await.is_err());
    assert!(matches!(
        execute(&peer, "edit", Some("new")).await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(memory.get_versioned(&key()).await.unwrap().unwrap().1, 1);
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        memory.get_versioned(&key()).await.unwrap().unwrap(),
        ("new".into(), 2)
    );
    assert_eq!(c.snapshot().await.unwrap().starts.len(), 1);
    qualify(&c, 2).await.unwrap();
}

#[tokio::test]
async fn control_authority_can_remove_metadata_for_a_closed_agent() {
    let store = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    qualify(&c, 1).await.unwrap();
    store.set(&key(), "card", None).await.unwrap();
    c.change(
        "close",
        AuthorityChange::CloseResource { resource: agent() },
        "operator",
        "maintenance",
    )
    .await
    .unwrap();
    begin(&c, "remove", 1, Some(1), None).await.unwrap();
    execute(&c, "remove", None).await.unwrap();
    assert!(store.get(&key()).await.unwrap().is_none());
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .closed_resources
            .contains(&agent())
    );
}

#[tokio::test]
async fn forged_completion_without_known_delivery_is_rejected_by_history_validation() {
    let store = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    begin(&c, "edit", 0, None, Some("new")).await.unwrap();
    let authority = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let mut raw = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    raw["changes"]["edit"]["pending"] = false.into();
    store
        .set(&authority, &serde_json::to_string(&raw).unwrap(), None)
        .await
        .unwrap();
    assert!(c.snapshot().await.is_err());
}

#[tokio::test]
async fn protocol_eleven_upgrade_preserves_registry_and_rejects_hybrid_mutation_intents() {
    for hybrid in [false, true] {
        let store = Arc::new(MemoryStateStore::new());
        let c = fixture(store.clone()).await;
        qualify(&c, 1).await.unwrap();
        if hybrid {
            begin(&c, "edit", 1, None, Some("new")).await.unwrap();
        }
        let before = c.snapshot().await.unwrap();
        let authority = StateKey::new(
            "city",
            "tenant",
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            "authority",
        );
        let mut raw = serde_json::to_value(&before).unwrap();
        raw["schema_version"] = 11.into();
        let serialized = serde_json::to_string(&raw).unwrap();
        store.set(&authority, &serialized, None).await.unwrap();
        assert!(
            AuthorityCoordinator::connect(store.clone(), "city", "tenant")
                .await
                .is_err()
        );
        let plan = AuthorityCoordinator::plan_scope_upgrade(
            store.clone(),
            "city",
            "tenant",
            ScopePurpose::Execution,
            "operator",
            "mutation upgrade",
        )
        .await;
        if hybrid {
            assert!(plan.is_err());
            assert_eq!(store.get(&authority).await.unwrap().unwrap(), serialized);
        } else {
            let plan = plan.unwrap();
            assert_eq!(plan.report().from_protocol, 11);
            assert_eq!(plan.report().to_protocol, 12);
            plan.apply(&plan.report().review_digest).await.unwrap();
            let after = c.snapshot().await.unwrap();
            assert_eq!(before.incarnation, after.incarnation);
            assert_eq!(before.agent_registry, after.agent_registry);
            assert_eq!(before.budget_parents, after.budget_parents);
        }
    }
}

#[tokio::test]
async fn lost_registration_acknowledgement_never_authorizes_a_duplicate_projection_write() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let memory = Arc::new(MemoryStateStore::new());
        let faults = Arc::new(FaultStore::new(memory.clone()));
        let c = fixture(faults.clone()).await;
        begin(&c, "edit", 0, None, Some("new")).await.unwrap();
        faults
            .fail_next(
                KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(execute(&c, "edit", Some("new")).await.is_err());
        assert!(memory.get(&key()).await.unwrap().is_none());
        if matches!(timing, FaultTiming::Before) {
            assert!(c.snapshot().await.unwrap().starts.is_empty());
            execute(&c, "edit", Some("new")).await.unwrap();
        } else {
            assert_eq!(
                c.snapshot()
                    .await
                    .unwrap()
                    .starts
                    .values()
                    .next()
                    .unwrap()
                    .status,
                AttemptStatus::InFlight
            );
            assert!(matches!(
                execute(&c, "edit", Some("new")).await,
                Err(CoordinationError::Restricted)
            ));
            assert!(memory.get(&key()).await.unwrap().is_none());
            assert!(qualify(&c, 1).await.is_err());
        }
    }
}

#[tokio::test]
async fn uncertain_deletion_is_not_certified_by_an_absent_projection() {
    let memory = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(memory.clone()));
    let c = fixture(faults.clone()).await;
    memory.set(&key(), "old", None).await.unwrap();
    qualify(&c, 1).await.unwrap();
    begin(&c, "delete", 1, Some(1), None).await.unwrap();
    faults
        .fail_next(
            KeyKind::BusAgentCard,
            WriteOperation::CompareAndDelete,
            FaultTiming::After,
        )
        .unwrap();
    assert!(execute(&c, "delete", None).await.is_err());
    assert!(memory.get(&key()).await.unwrap().is_none());
    assert!(matches!(
        execute(&c, "delete", None).await,
        Err(CoordinationError::Restricted)
    ));
    assert!(c.snapshot().await.unwrap().changes["delete"].pending);
    assert!(qualify(&c, 2).await.is_err());
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; isolated UUID registry mutation race"]
async fn independent_redis_registry_mutation_fences_qualification_and_duplicate_workers() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("registry-mutation-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    paused_delivery_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
    )
    .await;
}
