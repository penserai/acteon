use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::context::{
    AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
    RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::permit::{
    ExecutionPermit, PermitDenial, PermitIssuanceCeiling, PermitReference, PermittedAttempt,
    permit_revision_tag,
};
use acteon_governance::{
    AttemptStatus, AuthorityChange, AuthorityCoordinator, AuthorityStamp, COORDINATOR_KIND,
    CoordinationError, CoordinatorLimits, RootBudgetLimits, StartRegistration,
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use std::sync::Arc;

fn at(now: i64) -> acteon_time::ManualClock {
    acteon_time::ManualClock::new(chrono::DateTime::from_timestamp_millis(now).unwrap())
}
fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("actor", PrincipalKind::Agent).unwrap()
}
fn effect(op: &str, id: &str) -> AcceptedEffect {
    AcceptedEffect {
        operation: op.into(),
        resources: vec![
            ResourceRef::new(ResourceKind::Provider, "city", "tenant", id).unwrap(),
            ResourceRef::new(
                ResourceKind::Endpoint,
                "city",
                "tenant",
                format!("{id}-http"),
            )
            .unwrap(),
        ],
    }
}
fn permit() -> ExecutionPermit {
    ExecutionPermit {
        id: "permit".into(),
        revision: 1,
        subject: actor(),
        effects: vec![effect("read", "a"), effect("remediate", "b")],
        valid_from_ms: 0,
        limits: RootBudgetLimits {
            max_units: 10,
            max_concurrent: 2,
            deadline_ms: 2000,
        },
    }
}
fn ceiling() -> PermitIssuanceCeiling {
    PermitIssuanceCeiling {
        issuer: PrincipalIdentity::new("issuer", PrincipalKind::Human).unwrap(),
        subjects: vec![actor()],
        effects: permit().effects,
        valid_from_ms: 0,
        limits: RootBudgetLimits {
            max_units: 100,
            max_concurrent: 10,
            deadline_ms: 5000,
        },
    }
}
fn refs() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "permit".into(),
        accepted_revision: 1,
    }]
}
async fn publish(
    coordinator: &AuthorityCoordinator,
    updated: ExecutionPermit,
    id: &str,
) -> Result<acteon_governance::ChangeRecord, CoordinationError> {
    coordinator
        .publish_permit(
            id,
            updated.clone(),
            updated.revision - 1,
            &ceiling(),
            &coordinator.snapshot().await?.stamp(),
            "reviewed",
            10,
        )
        .await
}
async fn fixture(store: Arc<dyn StateStore>) -> (AuthorityCoordinator, VerifiedExecutionContext) {
    let coordinator = AuthorityCoordinator::initialize(
        store.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    publish(&coordinator, permit(), "issue").await.unwrap();
    let ctx = capture(
        store,
        &coordinator,
        &refs(),
        permit().effects,
        RootBudgetLimits {
            max_units: 10,
            max_concurrent: 2,
            deadline_ms: 1000,
        },
    )
    .await;
    (coordinator, ctx)
}
async fn capture(
    store: Arc<dyn StateStore>,
    coordinator: &AuthorityCoordinator,
    selected: &[PermitReference],
    accepted_effects: Vec<AcceptedEffect>,
    limits: RootBudgetLimits,
) -> VerifiedExecutionContext {
    let contexts = TrustedContextStore::new(
        store,
        coordinator.clone(),
        "domain".into(),
        "k1".into(),
        vec![ContextSigningKey::new("k1".into(), vec![1; 32]).unwrap()],
    )
    .unwrap();
    let ctx = contexts
        .capture_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: uuid::Uuid::new_v4(),
                    principal: actor(),
                    request_digest: "a".repeat(64),
                },
                credential_id: "key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: permit_revision_tag(selected).unwrap(),
                accepted_effects,
                deadline_ms: limits.deadline_ms,
                evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
            },
            100,
        )
        .await
        .unwrap();
    coordinator
        .create_root_budget(
            &ctx.execution_id().to_string(),
            "actor",
            limits,
            &coordinator.snapshot().await.unwrap().stamp(),
            100,
        )
        .await
        .unwrap();
    ctx
}

async fn start(
    coordinator: &AuthorityCoordinator,
    ctx: &VerifiedExecutionContext,
    id: &str,
    effect: &AcceptedEffect,
    now: i64,
) -> Result<StartRegistration, CoordinationError> {
    coordinator
        .register_permitted_attempt(PermittedAttempt {
            id,
            context: ctx,
            request_digest: &"a".repeat(64),
            permits: &refs(),
            effect,
            units: 1,
            clock: &at(now),
        })
        .await
}
#[tokio::test]
async fn current_narrowing_blocks_old_context_and_broadening_does_not_expand_it() {
    let (coordinator, ctx) = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut changed = permit();
    changed.revision = 2;
    changed.effects = vec![effect("remediate", "b")];
    publish(&coordinator, changed.clone(), "narrow")
        .await
        .unwrap();
    assert!(matches!(
        start(&coordinator, &ctx, "denied", &effect("read", "a"), 200).await,
        Err(CoordinationError::PermitDenied(PermitDenial::Effect))
    ));
    changed.revision = 3;
    changed.effects = permit().effects;
    publish(&coordinator, changed, "restore").await.unwrap();
    assert!(matches!(
        start(&coordinator, &ctx, "read", &effect("read", "a"), 200).await,
        Ok(StartRegistration::New(_))
    ));
    assert!(matches!(
        start(&coordinator, &ctx, "cross", &effect("read", "b"), 200).await,
        Err(CoordinationError::PermitDenied(PermitDenial::Effect))
    ));
}
#[tokio::test]
async fn binding_resource_order_replay_expiry_and_terminal_revocation() {
    let (coordinator, ctx) = fixture(Arc::new(MemoryStateStore::new())).await;
    let requested = effect("read", "a");
    let StartRegistration::New(record) = start(&coordinator, &ctx, "one", &requested, 200)
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut reversed = requested.clone();
    reversed.resources.reverse();
    assert!(matches!(
        start(&coordinator, &ctx, "one", &reversed, 300).await,
        Ok(StartRegistration::Existing(_))
    ));
    let mut other = refs();
    other[0].accepted_revision = 2;
    assert!(matches!(
        coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "spoof",
                context: &ctx,
                request_digest: &"a".repeat(64),
                permits: &other,
                effect: &requested,
                units: 1,
                clock: &at(200)
            })
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Binding))
    ));
    assert!(matches!(
        start(&coordinator, &ctx, "late", &requested, 1000).await,
        Err(CoordinationError::PermitDenied(PermitDenial::Validity))
    ));
    coordinator
        .change(
            "revoke",
            AuthorityChange::RevokePermit {
                permit_id: "permit".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    assert!(matches!(
        start(&coordinator, &ctx, "two", &requested, 300).await,
        Err(CoordinationError::PermitDenied(PermitDenial::Revoked))
    ));
    assert!(matches!(
        start(&coordinator, &ctx, "one", &requested, 300).await,
        Ok(StartRegistration::Existing(_))
    ));
    let mut replacement = permit();
    replacement.revision = 2;
    assert!(matches!(
        publish(&coordinator, replacement, "reactivate").await,
        Err(CoordinationError::PermitDenied(PermitDenial::Revoked))
    ));
    coordinator
        .settle("one", &record.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert_eq!(coordinator.snapshot().await.unwrap().starts.len(), 1);
}
#[tokio::test]
async fn narrowed_budget_is_checked_again_after_concurrent_cas_contention() {
    let (coordinator, ctx) = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut updated = permit();
    updated.revision = 2;
    updated.limits.max_units = 1;
    publish(&coordinator, updated, "limit").await.unwrap();
    let requested = effect("read", "a");
    let (first, second) = tokio::join!(
        start(&coordinator, &ctx, "a", &requested, 200),
        start(&coordinator, &ctx, "b", &requested, 200)
    );
    assert_eq!(
        usize::from(matches!(first, Ok(StartRegistration::New(_))))
            + usize::from(matches!(second, Ok(StartRegistration::New(_)))),
        1
    );
    assert!(
        matches!(
            first,
            Err(CoordinationError::PermitDenied(PermitDenial::Limits))
        ) || matches!(
            second,
            Err(CoordinationError::PermitDenied(PermitDenial::Limits))
        )
    );
    assert_eq!(
        coordinator.snapshot().await.unwrap().roots[&ctx.execution_id().to_string()].spent_units,
        1
    );
}
#[tokio::test]
async fn bounded_issuance_cannot_be_bypassed_or_change_a_permit_subject() {
    let (coordinator, _) = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut updated = permit();
    updated.revision = 2;
    updated.effects.push(effect("read", "b"));
    assert!(
        publish(&coordinator, updated.clone(), "expand")
            .await
            .is_err()
    );
    assert!(
        coordinator
            .change(
                "bypass",
                AuthorityChange::PublishPermit { permit: updated },
                "issuer",
                "bypass"
            )
            .await
            .is_err()
    );
    let mut updated = permit();
    updated.revision = 2;
    updated.subject = PrincipalIdentity::new("someone-else", PrincipalKind::Agent).unwrap();
    assert!(publish(&coordinator, updated, "retarget").await.is_err());
    let mut updated = permit();
    updated.revision = 2;
    updated.limits.max_units = 101;
    assert!(publish(&coordinator, updated, "units").await.is_err());
    assert_eq!(
        coordinator.snapshot().await.unwrap().permits["permit"]
            .permit
            .revision,
        1
    );
}
#[tokio::test]
async fn response_loss_retains_one_publication_and_replay_is_observation() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (coordinator, _) = fixture(store.clone()).await;
    let faults = Arc::new(FaultStore::new(store));
    let writer = AuthorityCoordinator::connect(faults.clone(), "city", "tenant")
        .await
        .unwrap();
    let mut updated = permit();
    updated.revision = 2;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        writer
            .publish_permit(
                "update",
                updated.clone(),
                1,
                &ceiling(),
                &stamp,
                "reviewed",
                200
            )
            .await
            .is_err()
    );
    let record = coordinator
        .publish_permit("update", updated, 1, &ceiling(), &stamp, "reviewed", 6000)
        .await
        .unwrap();
    assert!(record.pending);
    assert_eq!(coordinator.snapshot().await.unwrap().changes.len(), 2);
    coordinator.acknowledge_change("update").await.unwrap();
    assert_eq!(
        coordinator.snapshot().await.unwrap().permits["permit"]
            .permit
            .revision,
        2
    );
}
#[tokio::test]
async fn corrupted_current_records_cannot_disagree_with_retained_publications() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (coordinator, _) = fixture(store.clone()).await;
    let mut raw = serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap();
    raw["permits"]["permit"]["permit"]["limits"]["max_units"] = 100.into();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    store.set(&key, &raw.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store, "city", "tenant")
            .await
            .is_err()
    );
}
async fn controlled_race(
    store: Arc<dyn StateStore>,
    peer_store: Arc<dyn StateStore>,
    timing: FaultTiming,
    revoke: bool,
) {
    let faults = Arc::new(FaultStore::new(store));
    let (coordinator, ctx) = fixture(faults.clone()).await;
    let peer = AuthorityCoordinator::connect(peer_store.clone(), "city", "tenant")
        .await
        .unwrap();
    if !revoke {
        let mut updated = permit();
        updated.revision = 2;
        updated.limits.max_units = 1;
        publish(&peer, updated, "narrow-limit").await.unwrap();
    }
    let context_key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        ctx.reference().unwrap().context_id().to_string(),
    );
    let contender_context = ctx.clone();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            timing,
        )
        .unwrap();
    let task = tokio::spawn(async move {
        start(&coordinator, &ctx, "attempt", &effect("read", "a"), 200).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if revoke {
        peer.change(
            "revoke",
            AuthorityChange::RevokePermit {
                permit_id: "permit".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    } else {
        assert!(matches!(
            start(
                &peer,
                &contender_context,
                "winner",
                &effect("read", "a"),
                200
            )
            .await,
            Ok(StartRegistration::New(_))
        ));
    }
    release.send(()).unwrap();
    let result = task.await.unwrap();
    let state = peer.snapshot().await.unwrap();
    match (timing, revoke) {
        (FaultTiming::Before, true) => {
            assert!(matches!(result, Err(CoordinationError::StaleAuthority)));
            assert!(state.starts.is_empty());
        }
        (FaultTiming::After, true) => {
            assert!(matches!(result, Ok(StartRegistration::New(_))));
            assert_eq!(state.starts.len(), 1);
        }
        (FaultTiming::Before, false) => {
            assert!(matches!(
                result,
                Err(CoordinationError::PermitDenied(PermitDenial::Limits))
            ));
            assert_eq!(state.starts.len(), 1);
            assert_eq!(
                state.roots[&contender_context.execution_id().to_string()].spent_units,
                1
            );
        }
        _ => panic!("unsupported ordering"),
    }
    // Owned test fixture only; never delete live authority to reclaim capacity.
    peer_store.delete(&context_key).await.unwrap();
    peer_store
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
async fn controlled_permit_revocation_and_current_limit_races_are_atomic() {
    for (timing, revoke) in [
        (FaultTiming::Before, true),
        (FaultTiming::After, true),
        (FaultTiming::Before, false),
    ] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        controlled_race(store.clone(), store, timing, revoke).await;
    }
}
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; explicitly run against real Redis"]
async fn independent_redis_current_permits_pass_the_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("permit-contract-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let store: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let peer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    for (timing, revoke) in [
        (FaultTiming::Before, true),
        (FaultTiming::After, true),
        (FaultTiming::Before, false),
    ] {
        controlled_race(store.clone(), peer.clone(), timing, revoke).await;
    }
}

#[tokio::test]
async fn original_budget_ceiling_and_actual_input_binding_cannot_be_broadened() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (coordinator, ctx) = fixture(store.clone()).await;
    let mut updated = permit();
    updated.revision = 2;
    updated.limits.max_units = 20;
    publish(&coordinator, updated, "broaden-budget")
        .await
        .unwrap();
    let oversized = capture(
        store,
        &coordinator,
        &refs(),
        permit().effects,
        RootBudgetLimits {
            max_units: 11,
            max_concurrent: 2,
            deadline_ms: 1000,
        },
    )
    .await;
    assert!(matches!(
        start(
            &coordinator,
            &oversized,
            "oversized",
            &effect("read", "a"),
            200
        )
        .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Binding))
    ));
    assert!(matches!(
        coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "changed-input",
                context: &ctx,
                permits: &refs(),
                effect: &effect("read", "a"),
                request_digest: &"b".repeat(64),
                units: 1,
                clock: &at(200)
            })
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Binding))
    ));
    assert!(coordinator.snapshot().await.unwrap().starts.is_empty());
}
#[tokio::test]
async fn selected_permits_are_intersected_without_a_permission_cross_product() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let (coordinator, _) = fixture(store.clone()).await;
    let mut additional = permit();
    additional.id = "second".into();
    additional.effects = vec![effect("remediate", "b")];
    publish(&coordinator, additional, "second").await.unwrap();
    let mut selected = refs();
    selected.push(PermitReference {
        id: "second".into(),
        accepted_revision: 1,
    });
    let ctx = capture(
        store,
        &coordinator,
        &selected,
        permit().effects,
        RootBudgetLimits {
            max_units: 10,
            max_concurrent: 2,
            deadline_ms: 1000,
        },
    )
    .await;
    assert!(matches!(
        coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "union",
                context: &ctx,
                permits: &selected,
                effect: &effect("read", "a"),
                request_digest: &"a".repeat(64),
                units: 1,
                clock: &at(200)
            })
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Effect))
    ));
    assert!(coordinator.snapshot().await.unwrap().starts.is_empty());
}

#[tokio::test]
async fn permitted_root_capture_repairs_interruption_without_reallocating_or_spending() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let (coordinator, _) = fixture(store.clone()).await;
        let faults = Arc::new(FaultStore::new(store));
        let scoped = AuthorityCoordinator::connect(faults.clone(), "city", "tenant")
            .await
            .unwrap();
        let contexts = TrustedContextStore::new(
            faults.clone(),
            scoped,
            "domain".into(),
            "k1".into(),
            vec![ContextSigningKey::new("k1".into(), vec![1; 32]).unwrap()],
        )
        .unwrap();
        let admission = RootContextAdmission {
            handle: ExecutionContextHandle::new(),
            binding: ContextBinding {
                execution_id: uuid::Uuid::new_v4(),
                principal: actor(),
                request_digest: "a".repeat(64),
            },
            credential_id: "key".into(),
            auth_method: "api_key".into(),
            accepted_ceiling_revision: "placeholder".into(),
            accepted_effects: permit().effects,
            deadline_ms: 1000,
            evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
        };
        let limits = RootBudgetLimits {
            max_units: 3,
            max_concurrent: 1,
            deadline_ms: 1000,
        };
        faults
            .fail_next(
                KeyKind::Custom(COORDINATOR_KIND.into()),
                WriteOperation::CompareAndSwap,
                timing,
            )
            .unwrap();
        assert!(
            contexts
                .capture_permitted_root(admission.clone(), &refs(), limits.clone(), &at(100))
                .await
                .is_err()
        );
        let captured = contexts
            .capture_permitted_root(admission, &refs(), limits.clone(), &at(200))
            .await
            .unwrap();
        let state = coordinator.snapshot().await.unwrap();
        assert_eq!(
            state.roots[&captured.execution_id().to_string()].limits,
            limits
        );
        assert_eq!(
            state.roots[&captured.execution_id().to_string()].spent_units,
            0
        );
        assert!(matches!(
            start(&coordinator, &captured, "actual", &effect("read", "a"), 200).await,
            Ok(StartRegistration::New(_))
        ));
    }
}

#[tokio::test]
async fn deadline_is_refreshed_after_budget_cas_contention() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(store.clone()));
    let (coordinator, ctx) = fixture(faults.clone()).await;
    let peer = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap();
    let clock = Arc::new(at(200));
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let saved = ctx.clone();
    let time = clock.clone();
    let task = tokio::spawn(async move {
        coordinator
            .register_permitted_attempt(PermittedAttempt {
                id: "delayed",
                context: &ctx,
                permits: &refs(),
                effect: &effect("read", "a"),
                request_digest: &"a".repeat(64),
                units: 1,
                clock: time.as_ref(),
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    start(&peer, &saved, "winner", &effect("read", "a"), 200)
        .await
        .unwrap();
    clock
        .advance_to(std::time::Duration::from_millis(800))
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::PermitDenied(PermitDenial::Validity))
    ));
    assert_eq!(peer.snapshot().await.unwrap().starts.len(), 1);
}

#[tokio::test]
async fn evaluated_publication_checks_current_authority_before_replay() {
    let coordinator = AuthorityCoordinator::initialize(
        Arc::new(MemoryStateStore::new()),
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
    let policy = ceiling();
    let time = at(100);
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let publish = |stamp: AuthorityStamp| {
        let coordinator = coordinator.clone();
        let time = time.clone();
        async move {
            coordinator
                .publish_permit_evaluated(acteon_governance::permit::EvaluatedPermitPublication {
                    change_id: "issue",
                    permit: permit(),
                    expected_revision: 0,
                    ceiling: &ceiling(),
                    evaluated_authority: &stamp,
                    reason: "reviewed",
                    clock: &time,
                })
                .await
        }
    };
    let record = publish(stamp.clone()).await.unwrap();
    assert!(matches!(
        publish(stamp).await,
        Err(CoordinationError::StaleAuthority)
    ));
    assert_eq!(
        publish(coordinator.snapshot().await.unwrap().stamp())
            .await
            .unwrap(),
        record
    );
    coordinator
        .change(
            "revoke-issuer",
            AuthorityChange::RevokeSubject {
                subject: policy.issuer.id().into(),
            },
            "security",
            "revoked",
        )
        .await
        .unwrap();
    assert!(matches!(
        publish(coordinator.snapshot().await.unwrap().stamp()).await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn evaluated_publication_resamples_expiry_after_same_generation_cas_conflict() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(store.clone()));
    let coordinator = AuthorityCoordinator::initialize(
        faults.clone(),
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
    coordinator
        .change(
            "existing-event",
            AuthorityChange::CloseResource {
                resource: ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "unrelated")
                    .unwrap(),
            },
            "security",
            "maintenance",
        )
        .await
        .unwrap();
    let peer = AuthorityCoordinator::connect(store, "city", "tenant")
        .await
        .unwrap();
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let original_stamp = stamp.clone();
    let clock = at(100);
    let task_clock = clock.clone();
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let task = tokio::spawn(async move {
        coordinator
            .publish_permit_evaluated(acteon_governance::permit::EvaluatedPermitPublication {
                change_id: "delayed-issue",
                permit: permit(),
                expected_revision: 0,
                ceiling: &ceiling(),
                evaluated_authority: &stamp,
                reason: "reviewed",
                clock: &task_clock,
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    peer.acknowledge_change("existing-event").await.unwrap();
    assert_eq!(peer.snapshot().await.unwrap().stamp(), original_stamp);
    clock
        .advance_to(std::time::Duration::from_millis(6000))
        .unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    let state = peer.snapshot().await.unwrap();
    assert!(state.permits.is_empty());
    assert!(!state.changes.contains_key("delayed-issue"));
}

#[tokio::test]
async fn root_limit_projection_intersects_all_permits_without_granting_authority() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let coordinator =
        AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
            .await
            .unwrap();
    coordinator
        .reserve_scope(acteon_governance::ScopePurpose::Execution)
        .await
        .unwrap();
    publish(&coordinator, permit(), "issue").await.unwrap();
    let mut narrower = permit();
    narrower.id = "narrower".into();
    narrower.limits = RootBudgetLimits {
        max_units: 3,
        max_concurrent: 1,
        deadline_ms: 1500,
    };
    publish(&coordinator, narrower, "issue-narrower")
        .await
        .unwrap();
    let mut references = refs();
    references.push(PermitReference {
        id: "narrower".into(),
        accepted_revision: 1,
    });
    let mut admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor(),
            request_digest: "a".repeat(64),
        },
        credential_id: "key".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "placeholder".into(),
        accepted_effects: permit().effects,
        deadline_ms: 3000,
        evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
    };
    let ceiling = RootBudgetLimits {
        max_units: 50,
        max_concurrent: 10,
        deadline_ms: 3000,
    };
    let limits = coordinator
        .attenuate_permitted_root_limits(&admission, &references, ceiling.clone(), 100)
        .await
        .unwrap();
    assert_eq!(
        limits,
        RootBudgetLimits {
            max_units: 3,
            max_concurrent: 1,
            deadline_ms: 1500
        }
    );
    assert_eq!(coordinator.snapshot().await.unwrap().roots.len(), 0); // Projection allocated no root.
    admission.binding.principal = PrincipalIdentity::new("other", PrincipalKind::Agent).unwrap();
    assert!(matches!(
        coordinator
            .attenuate_permitted_root_limits(&admission, &references, ceiling.clone(), 100)
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Subject))
    ));
    admission.binding.principal = actor();
    admission.accepted_effects = vec![effect("ungranted", "c")];
    assert!(matches!(
        coordinator
            .attenuate_permitted_root_limits(&admission, &references, ceiling.clone(), 100)
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Effect))
    ));
    admission.accepted_effects = permit().effects;
    assert!(matches!(
        coordinator
            .attenuate_permitted_root_limits(&admission, &references, ceiling.clone(), 1500)
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Limits))
    ));
    coordinator
        .change(
            "revoke-narrower",
            AuthorityChange::RevokePermit {
                permit_id: "narrower".into(),
                expected_revision: 1,
            },
            "issuer",
            "stop",
        )
        .await
        .unwrap();
    // Refresh the stamp to exercise permit revocation rather than stale evaluation.
    admission.evaluated_authority = coordinator.snapshot().await.unwrap().stamp();
    assert!(matches!(
        coordinator
            .attenuate_permitted_root_limits(&admission, &references, ceiling, 100)
            .await,
        Err(CoordinationError::PermitDenied(PermitDenial::Revoked))
    ));
}

#[tokio::test]
async fn operation_seals_are_atomic_immutable_and_never_attached_to_legacy_replays() {
    use acteon_governance::AttemptEvidenceReference;
    for sealed in [false, true] {
        let (coordinator, ctx) = fixture(Arc::new(MemoryStateStore::new())).await;
        let selected = refs();
        let footprint = effect("read", "a");
        let digest = "a".repeat(64);
        let clock = at(200);
        let request = || PermittedAttempt {
            id: "original",
            context: &ctx,
            permits: &selected,
            effect: &footprint,
            request_digest: &digest,
            units: 1,
            clock: &clock,
        };
        let seal = AttemptEvidenceReference {
            id: ctx.execution_id().to_string(),
            digest: "b".repeat(64),
        };
        if sealed {
            coordinator
                .register_permitted_attempt_with_operation(request(), &seal)
                .await
                .unwrap();
            assert!(matches!(
                coordinator
                    .register_permitted_attempt_with_operation(request(), &seal)
                    .await
                    .unwrap(),
                StartRegistration::Existing(_)
            ));
        } else {
            coordinator
                .register_permitted_attempt(request())
                .await
                .unwrap();
        }
        let before = coordinator.snapshot().await.unwrap();
        assert_eq!(
            before.starts["original"].operation_evidence,
            sealed.then_some(seal.clone())
        );
        let changed = AttemptEvidenceReference {
            digest: "c".repeat(64),
            ..seal.clone()
        };
        assert!(matches!(
            coordinator
                .register_permitted_attempt_with_operation(request(), &changed)
                .await,
            Err(CoordinationError::Conflict)
        ));
        if sealed {
            assert!(matches!(
                coordinator.register_permitted_attempt(request()).await,
                Err(CoordinationError::Conflict)
            ));
        } else {
            assert!(matches!(
                coordinator
                    .register_permitted_attempt_with_operation(request(), &seal)
                    .await,
                Err(CoordinationError::Conflict)
            ));
        }
        let wrong_owner = AttemptEvidenceReference {
            id: uuid::Uuid::new_v4().to_string(),
            ..seal
        };
        assert!(matches!(
            coordinator
                .register_permitted_attempt_with_operation(request(), &wrong_owner)
                .await,
            Err(CoordinationError::Invalid(_))
        ));
        let after = coordinator.snapshot().await.unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(&after).unwrap()
        );
        let root = &after.roots[&ctx.execution_id().to_string()];
        assert_eq!((root.spent_units, root.active_attempts), (1, 1));
    }
}
