//! Accounting contracts, not proof that a raw budget reservation authorizes a tool.
use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::context::{
    AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
    RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::permit::{
    ExecutionPermit, PermitIssuanceCeiling, PermitReference, permit_revision_tag,
};
use acteon_governance::{
    AttemptRequest, AttemptStatus, AuthorityCoordinator, ChildBudgetAdmission, CoordinationError,
    CoordinatorLimits, RootBudgetLimits, RootReservation, ScopePurpose, StartRegistration,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use std::sync::Arc;
fn actor() -> PrincipalIdentity {
    PrincipalIdentity::new("worker", PrincipalKind::Agent).unwrap()
}
fn clock() -> acteon_time::ManualClock {
    acteon_time::ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
}
fn effect() -> AcceptedEffect {
    AcceptedEffect {
        operation: "execute".into(),
        resources: vec![
            ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", "incident").unwrap(),
        ],
    }
}
fn refs() -> Vec<PermitReference> {
    vec![PermitReference {
        id: "work".into(),
        accepted_revision: 1,
    }]
}
fn limits(units: u64) -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: units,
        max_concurrent: 1,
        deadline_ms: 1000,
    }
}
async fn fixture() -> (
    Arc<dyn StateStore>,
    AuthorityCoordinator,
    TrustedContextStore,
    VerifiedExecutionContext,
) {
    fixture_with_store(Arc::new(MemoryStateStore::new())).await
}
async fn fixture_with_store(
    state: Arc<dyn StateStore>,
) -> (
    Arc<dyn StateStore>,
    AuthorityCoordinator,
    TrustedContextStore,
    VerifiedExecutionContext,
) {
    fixture_with_limits(state, limits(2)).await
}
async fn fixture_with_limits(
    state: Arc<dyn StateStore>,
    root_limits: RootBudgetLimits,
) -> (
    Arc<dyn StateStore>,
    AuthorityCoordinator,
    TrustedContextStore,
    VerifiedExecutionContext,
) {
    let permit_limits = RootBudgetLimits {
        max_concurrent: root_limits.max_concurrent,
        ..limits(10)
    };
    let c = AuthorityCoordinator::initialize(
        state.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    c.publish_permit(
        "issue",
        ExecutionPermit {
            id: "work".into(),
            revision: 1,
            subject: actor(),
            effects: vec![effect()],
            valid_from_ms: 0,
            limits: permit_limits.clone(),
        },
        0,
        &PermitIssuanceCeiling {
            issuer: actor(),
            subjects: vec![actor()],
            effects: vec![effect()],
            valid_from_ms: 0,
            limits: permit_limits.clone(),
        },
        &c.snapshot().await.unwrap().stamp(),
        "work",
        100,
    )
    .await
    .unwrap();
    let contexts = TrustedContextStore::new(
        state.clone(),
        c.clone(),
        "test".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
    )
    .unwrap();
    let parent = contexts
        .capture_permitted_root(
            admission(&c, uuid::Uuid::new_v4()).await,
            &refs(),
            root_limits,
            &clock(),
        )
        .await
        .unwrap();
    (state, c, contexts, parent)
}
async fn admission(c: &AuthorityCoordinator, id: uuid::Uuid) -> RootContextAdmission {
    RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: id,
            principal: actor(),
            request_digest: "a".repeat(64),
        },
        credential_id: "key".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: permit_revision_tag(&refs()).unwrap(),
        accepted_effects: vec![effect()],
        deadline_ms: 1000,
        evaluated_authority: c.snapshot().await.unwrap().stamp(),
    }
}
async fn child(
    c: &AuthorityCoordinator,
    parent: &VerifiedExecutionContext,
    units: u64,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    c.create_child_budget(ChildBudgetAdmission {
        parent,
        permits: &refs(),
        execution_id: id,
        limits: limits(units),
        clock: &clock(),
    })
    .await
    .unwrap();
    id
}
async fn start(
    c: &AuthorityCoordinator,
    id: &str,
    ledger: uuid::Uuid,
) -> Result<StartRegistration, CoordinationError> {
    c.register_attempt(AttemptRequest {
        id,
        subject: actor().id(),
        resources: &effect().resources,
        request_digest: "actual-input",
        expected_authority: &c.snapshot().await?.stamp(),
        reservation: Some(RootReservation {
            root_id: ledger.to_string(),
            units: 1,
        }),
        now_ms: 100,
    })
    .await
}
#[tokio::test]
async fn siblings_share_units_concurrency_and_idempotent_settlement() {
    let (state, c, _, parent) = fixture().await;
    let peer = AuthorityCoordinator::connect(state, "city", "tenant")
        .await
        .unwrap();
    let left = child(&c, &parent, 2).await;
    let right = child(&c, &parent, 2).await;
    let (a, b) = tokio::join!(start(&c, "left", left), start(&peer, "right", right));
    let (id, first, other) = match (a, b) {
        (Ok(StartRegistration::New(record)), Err(CoordinationError::ConcurrencyExhausted)) => {
            ("left", record, right)
        }
        (Err(CoordinationError::ConcurrencyExhausted), Ok(StartRegistration::New(record))) => {
            ("right", record, left)
        }
        outcome => panic!("exactly one start must win: {outcome:?}"),
    };
    c.settle(id, &first.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    assert!(matches!(
        start(&peer, "blocked", other).await,
        Err(CoordinationError::ConcurrencyExhausted)
    ));
    peer.settle(id, &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    c.settle(id, &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let StartRegistration::New(second) = start(&peer, "second", other).await.unwrap() else {
        panic!("new start")
    };
    c.settle("second", &second.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert!(matches!(
        start(&peer, "exhausted", other).await,
        Err(CoordinationError::BudgetExhausted)
    ));
    let snapshot = peer.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&parent.execution_id().to_string()].spent_units,
        2
    );
    assert_eq!(snapshot.roots[&left.to_string()].spent_units, 1);
    assert_eq!(snapshot.roots[&right.to_string()].spent_units, 1);
    assert!(snapshot.roots.values().all(|r| r.active_attempts == 0));
}
#[tokio::test]
async fn child_replay_cannot_reparent_increase_limits_or_become_a_fresh_root() {
    let (_, c, contexts, parent) = fixture().await;
    let id = child(&c, &parent, 1).await;
    let same = c
        .create_child_budget(ChildBudgetAdmission {
            parent: &parent,
            permits: &refs(),
            execution_id: id,
            limits: limits(1),
            clock: &clock(),
        })
        .await
        .unwrap();
    assert_eq!(same.spent_units, 0);
    assert!(matches!(
        c.create_child_budget(ChildBudgetAdmission {
            parent: &parent,
            permits: &refs(),
            execution_id: id,
            limits: limits(2),
            clock: &clock()
        })
        .await,
        Err(CoordinationError::Conflict)
    ));
    assert!(matches!(
        c.create_root_budget(
            &id.to_string(),
            actor().id(),
            limits(1),
            &c.snapshot().await.unwrap().stamp(),
            100
        )
        .await,
        Err(CoordinationError::Conflict)
    ));
    let other_parent = contexts
        .capture_permitted_root(
            admission(&c, uuid::Uuid::new_v4()).await,
            &refs(),
            limits(2),
            &clock(),
        )
        .await
        .unwrap();
    assert!(matches!(
        c.create_child_budget(ChildBudgetAdmission {
            parent: &other_parent,
            permits: &refs(),
            execution_id: id,
            limits: limits(1),
            clock: &clock(),
        })
        .await,
        Err(CoordinationError::Conflict)
    ));
    assert_eq!(c.snapshot().await.unwrap().roots.len(), 3);
}
#[tokio::test]
async fn malformed_parent_graph_and_accounting_are_rejected_on_recovery() {
    let (store, c, _, parent) = fixture().await;
    let id = child(&c, &parent, 1).await;
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    let original = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    for mutation in ["cycle", "missing", "widened", "counter"] {
        let mut raw = original.clone();
        match mutation {
            "cycle" => raw["budget_parents"][id.to_string()] = id.to_string().into(),
            "missing" => raw["budget_parents"][id.to_string()] = "missing".into(),
            "widened" => raw["roots"][id.to_string()]["limits"]["max_units"] = 3.into(),
            "counter" => raw["roots"][parent.execution_id().to_string()]["spent_units"] = 1.into(),
            _ => unreachable!(),
        }
        store.set(&key, &raw.to_string(), None).await.unwrap();
        assert!(c.snapshot().await.is_err(), "{mutation}");
    }
}

async fn capture_child(
    contexts: &TrustedContextStore,
    parent: &VerifiedExecutionContext,
    key: &str,
    digest: char,
) -> VerifiedExecutionContext {
    contexts
        .capture_child(acteon_governance::context::ChildContextAdmission {
            admission_key: key,
            parent,
            handle: ExecutionContextHandle::new(),
            execution_id: uuid::Uuid::new_v4(),
            request_digest: digest.to_string().repeat(64),
            accepted_effects: vec![effect()],
            restrictions: vec![],
            permits: &refs(),
            limits: limits(1),
            clock: &clock(),
        })
        .await
        .unwrap()
}
async fn permitted(
    c: &AuthorityCoordinator,
    ctx: &VerifiedExecutionContext,
    id: &str,
) -> Result<StartRegistration, CoordinationError> {
    c.register_permitted_attempt(acteon_governance::permit::PermittedAttempt {
        id,
        context: ctx,
        permits: &refs(),
        effect: &effect(),
        request_digest: ctx.reference().unwrap().request_digest(),
        units: 1,
        clock: &clock(),
    })
    .await
}
#[tokio::test]
async fn signed_children_and_grandchildren_retain_one_root_and_branch_ceiling() {
    let (_, c, contexts, parent) = fixture().await;
    let branch = capture_child(&contexts, &parent, "branch", 'b').await;
    let grandchild = capture_child(&contexts, &branch, "grandchild", 'c').await;
    let another = capture_child(&contexts, &branch, "another-grandchild", 'd').await;
    assert_eq!(grandchild.root_execution_id(), parent.execution_id());
    assert_eq!(
        grandchild.parent_reference().unwrap(),
        &branch.reference().unwrap()
    );
    let StartRegistration::New(first) = permitted(&c, &grandchild, "first-grandchild")
        .await
        .unwrap()
    else {
        panic!("new")
    };
    c.settle("first-grandchild", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert!(matches!(
        permitted(&c, &another, "branch-exhausted").await,
        Err(CoordinationError::BudgetExhausted)
    ));
    let sibling = capture_child(&contexts, &parent, "sibling", 'e').await;
    assert!(matches!(
        permitted(&c, &sibling, "other-branch").await,
        Ok(StartRegistration::New(_))
    ));
    let state = c.snapshot().await.unwrap();
    assert_eq!(
        state.roots[&parent.execution_id().to_string()].spent_units,
        2
    );
    assert_eq!(
        state.roots[&branch.execution_id().to_string()].spent_units,
        1
    );
}
#[tokio::test]
async fn child_admission_lost_ack_recovers_original_identity_on_another_replica() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let (state, c, _, parent) = fixture().await;
    let faults = Arc::new(FaultStore::new(state.clone()));
    let contexts = TrustedContextStore::new(
        faults.clone(),
        c.clone(),
        "test".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
    )
    .unwrap();
    let original_id = uuid::Uuid::new_v4();
    faults
        .fail_next(
            KeyKind::Custom(acteon_governance::context::CHILD_ADMISSION_KIND.into()),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        contexts
            .capture_child(acteon_governance::context::ChildContextAdmission {
                admission_key: "lost-child",
                parent: &parent,
                handle: ExecutionContextHandle::new(),
                execution_id: original_id,
                request_digest: "b".repeat(64),
                accepted_effects: vec![effect()],
                restrictions: vec![],
                permits: &refs(),
                limits: limits(1),
                clock: &clock(),
            })
            .await
            .is_err()
    );
    assert_eq!(c.snapshot().await.unwrap().roots.len(), 1);
    let peer = AuthorityCoordinator::connect(state.clone(), "city", "tenant")
        .await
        .unwrap();
    let replacement = TrustedContextStore::new(
        state,
        peer.clone(),
        "test".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
    )
    .unwrap();
    let restored = capture_child(&replacement, &parent, "lost-child", 'b').await;
    assert_eq!(restored.execution_id(), original_id);
    let retry = capture_child(&contexts, &parent, "lost-child", 'b').await;
    assert_eq!(retry.reference().unwrap(), restored.reference().unwrap());
    assert_eq!(peer.snapshot().await.unwrap().roots.len(), 2);
    let recovered = replacement
        .recover_reference(&restored.reference().unwrap(), 100)
        .await
        .unwrap();
    assert_eq!(recovered.root_execution_id(), parent.execution_id());
    assert_eq!(recovered.parent_reference(), restored.parent_reference());
}
#[tokio::test]
async fn child_binding_refuses_unaccepted_effects_changed_input_and_reparented_payer() {
    let (store, c, contexts, parent) = fixture().await;
    let child = capture_child(&contexts, &parent, "child", 'b').await;
    let roots = c.snapshot().await.unwrap().roots.len();
    let mut unaccepted = effect();
    unaccepted.operation = "unaccepted".into();
    for (key, effect, digest) in [("unaccepted", unaccepted, 'b'), ("child", effect(), 'c')] {
        assert!(
            contexts
                .capture_child(acteon_governance::context::ChildContextAdmission {
                    admission_key: key,
                    parent: &parent,
                    handle: ExecutionContextHandle::new(),
                    execution_id: uuid::Uuid::new_v4(),
                    request_digest: digest.to_string().repeat(64),
                    accepted_effects: vec![effect],
                    restrictions: vec![],
                    permits: &refs(),
                    limits: limits(1),
                    clock: &clock(),
                })
                .await
                .is_err()
        );
    }
    assert_eq!(c.snapshot().await.unwrap().roots.len(), roots);
    let unrelated = contexts
        .capture_permitted_root(
            admission(&c, uuid::Uuid::new_v4()).await,
            &refs(),
            limits(2),
            &clock(),
        )
        .await
        .unwrap();
    let mut raw = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    raw["budget_parents"][child.execution_id().to_string()] =
        unrelated.execution_id().to_string().into();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    store.set(&key, &raw.to_string(), None).await.unwrap();
    assert!(c.snapshot().await.is_ok()); // Accounting is still valid; signed ancestry is not.
    assert!(matches!(
        permitted(&c, &child, "wrong-payer").await,
        Err(CoordinationError::Conflict)
    ));
    assert!(c.snapshot().await.unwrap().starts.is_empty());
}
#[tokio::test]
async fn current_permit_limits_apply_to_the_whole_root_across_siblings() {
    let (_, c, contexts, parent) = fixture().await;
    let left = capture_child(&contexts, &parent, "left", 'b').await;
    let right = capture_child(&contexts, &parent, "right", 'c').await;
    let StartRegistration::New(first) = permitted(&c, &left, "first").await.unwrap() else {
        panic!("new")
    };
    c.settle("first", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let mut narrowed = c.snapshot().await.unwrap().permits["work"].permit.clone();
    narrowed.revision = 2;
    narrowed.limits.max_units = 1;
    c.publish_permit(
        "narrow",
        narrowed,
        1,
        &PermitIssuanceCeiling {
            issuer: actor(),
            subjects: vec![actor()],
            effects: vec![effect()],
            valid_from_ms: 0,
            limits: limits(10),
        },
        &c.snapshot().await.unwrap().stamp(),
        "narrow root entitlement",
        100,
    )
    .await
    .unwrap();
    assert!(matches!(
        permitted(&c, &right, "second").await,
        Err(CoordinationError::PermitDenied(
            acteon_governance::permit::PermitDenial::Limits
        ))
    ));
    assert_eq!(
        c.snapshot().await.unwrap().roots[&parent.execution_id().to_string()].spent_units,
        1
    );
}
#[tokio::test]
async fn actor_revocation_blocks_child_replay_and_new_effects_without_spending() {
    let (_, c, contexts, parent) = fixture().await;
    let child = capture_child(&contexts, &parent, "child", 'b').await;
    c.change(
        "revoke",
        acteon_governance::AuthorityChange::RevokeSubject {
            subject: actor().id().into(),
        },
        "security",
        "offboarding",
    )
    .await
    .unwrap();
    assert!(
        contexts
            .capture_child(acteon_governance::context::ChildContextAdmission {
                admission_key: "child",
                parent: &parent,
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                request_digest: "b".repeat(64),
                accepted_effects: vec![effect()],
                restrictions: vec![],
                permits: &refs(),
                limits: limits(1),
                clock: &clock(),
            })
            .await
            .is_err()
    );
    assert!(matches!(
        permitted(&c, &child, "after-offboarding").await,
        Err(CoordinationError::Restricted)
    ));
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .roots
            .values()
            .all(|r| r.spent_units == 0)
    );
}

async fn durable_descendant_contract(a: Arc<dyn StateStore>, b: Arc<dyn StateStore>) {
    let (_, coordinator, contexts, parent) = fixture_with_store(a).await;
    let child = capture_child(&contexts, &parent, "durable-child", 'b').await;
    let sibling = capture_child(&contexts, &parent, "durable-sibling", 'c').await;
    let peer = AuthorityCoordinator::connect(b.clone(), "city", "tenant")
        .await
        .unwrap();
    let recovered_contexts = TrustedContextStore::new(
        b,
        peer.clone(),
        "test".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
    )
    .unwrap();
    let recovered = recovered_contexts
        .recover_reference(&child.reference().unwrap(), 100)
        .await
        .unwrap();
    assert_eq!(recovered.root_execution_id(), parent.execution_id());
    assert_eq!(
        recovered.parent_reference(),
        Some(&parent.reference().unwrap())
    );
    let StartRegistration::New(first) =
        permitted(&peer, &recovered, "durable-first").await.unwrap()
    else {
        panic!("new start")
    };
    coordinator
        .settle("durable-first", &first.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    assert!(matches!(
        permitted(&peer, &sibling, "durable-blocked").await,
        Err(CoordinationError::ConcurrencyExhausted)
    ));
    peer.settle("durable-first", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    coordinator
        .settle("durable-first", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let StartRegistration::New(second) = permitted(&coordinator, &sibling, "durable-second")
        .await
        .unwrap()
    else {
        panic!("new sibling start")
    };
    peer.settle("durable-second", &second.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let snapshot = peer.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&parent.execution_id().to_string()].spent_units,
        2
    );
    assert!(snapshot.roots.values().all(|r| r.active_attempts == 0));
    assert_eq!(snapshot.starts.len(), 2);
    assert_eq!(snapshot.budget_parents.len(), 2);
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL"]
async fn independent_redis_descendant_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("descendants-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    durable_descendant_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn independent_postgres_descendant_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("descendants_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    durable_descendant_contract(
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap()),
    )
    .await;
    let pool = sqlx::PgPool::connect(&config.url).await.unwrap();
    for suffix in ["state", "locks", "timeout_index", "chain_ready_index"] {
        sqlx::query(&format!(
            "DROP TABLE public.{}{suffix}",
            config.table_prefix
        ))
        .execute(&pool)
        .await
        .unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn context_and_budget_lost_ack_repair_preserves_first_child_identity() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    for (kind, operation) in [
        (
            acteon_governance::context::CONTEXT_KIND,
            WriteOperation::CheckAndSet,
        ),
        (
            acteon_governance::COORDINATOR_KIND,
            WriteOperation::CompareAndSwap,
        ),
    ] {
        let (state, c, _, parent) = fixture().await;
        let faults = Arc::new(FaultStore::new(state.clone()));
        let fault_coordinator = AuthorityCoordinator::connect(faults.clone(), "city", "tenant")
            .await
            .unwrap();
        let contexts = TrustedContextStore::new(
            faults.clone(),
            fault_coordinator,
            "test".into(),
            "key".into(),
            vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
        )
        .unwrap();
        faults
            .fail_next(KeyKind::Custom(kind.into()), operation, FaultTiming::After)
            .unwrap();
        let original_id = uuid::Uuid::new_v4();
        let original_handle = ExecutionContextHandle::new();
        assert!(
            contexts
                .capture_child(acteon_governance::context::ChildContextAdmission {
                    admission_key: "child-repair",
                    parent: &parent,
                    handle: original_handle.clone(),
                    execution_id: original_id,
                    request_digest: "b".repeat(64),
                    accepted_effects: vec![effect()],
                    restrictions: vec![],
                    permits: &refs(),
                    limits: limits(1),
                    clock: &clock(),
                })
                .await
                .is_err()
        );
        let replacement = TrustedContextStore::new(
            state,
            c.clone(),
            "test".into(),
            "key".into(),
            vec![ContextSigningKey::new("key".into(), vec![3; 32]).unwrap()],
        )
        .unwrap();
        let restored = capture_child(&replacement, &parent, "child-repair", 'b').await;
        assert_eq!(restored.execution_id(), original_id);
        assert_eq!(restored.handle(), &original_handle);
        let snapshot = c.snapshot().await.unwrap();
        assert_eq!(snapshot.roots.len(), 2);
        assert_eq!(snapshot.budget_parents.len(), 1);
        assert!(snapshot.starts.is_empty());
    }
}

#[tokio::test]
async fn descendant_depth_and_count_are_bounded_without_spending() {
    let (_, c, contexts, parent) = fixture().await;
    let mut ancestor = parent.clone();
    for depth in 1..acteon_governance::MAX_BUDGET_DEPTH {
        ancestor = capture_child(&contexts, &ancestor, &format!("depth-{depth}"), 'b').await;
    }
    assert!(matches!(
        c.create_child_budget(ChildBudgetAdmission {
            parent: &ancestor,
            permits: &refs(),
            execution_id: uuid::Uuid::new_v4(),
            limits: limits(1),
            clock: &clock(),
        })
        .await,
        Err(CoordinationError::Capacity)
    ));
    let existing = c.snapshot().await.unwrap().budget_parents.len();
    for _ in existing..acteon_governance::MAX_ROOT_DESCENDANTS {
        child(&c, &parent, 1).await;
    }
    assert!(matches!(
        c.create_child_budget(ChildBudgetAdmission {
            parent: &parent,
            permits: &refs(),
            execution_id: uuid::Uuid::new_v4(),
            limits: limits(1),
            clock: &clock(),
        })
        .await,
        Err(CoordinationError::Capacity)
    ));
    let snapshot = c.snapshot().await.unwrap();
    assert_eq!(
        snapshot.budget_parents.len(),
        acteon_governance::MAX_ROOT_DESCENDANTS
    );
    assert!(snapshot.roots.values().all(|r| r.spent_units == 0));
    assert!(snapshot.starts.is_empty());
}

#[tokio::test]
async fn child_allocation_resamples_expiry_after_same_generation_cas_conflict() {
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let (state, peer, _, parent) = fixture().await;
    let faults = Arc::new(FaultStore::new(state));
    let c = AuthorityCoordinator::connect(faults.clone(), "city", "tenant")
        .await
        .unwrap();
    let time = clock();
    let task_time = time.clone();
    let stamp = peer.snapshot().await.unwrap().stamp();
    let release = faults
        .pause_next(
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let task = tokio::spawn(async move {
        c.create_child_budget(ChildBudgetAdmission {
            parent: &parent,
            permits: &refs(),
            execution_id: uuid::Uuid::new_v4(),
            limits: limits(1),
            clock: &task_time,
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
    peer.acknowledge_change("issue").await.unwrap();
    assert_eq!(peer.snapshot().await.unwrap().stamp(), stamp);
    time.advance_to(std::time::Duration::from_millis(1000))
        .unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    let snapshot = peer.snapshot().await.unwrap();
    assert_eq!(snapshot.roots.len(), 1);
    assert!(snapshot.budget_parents.is_empty());
}

#[tokio::test]
async fn signed_child_recovery_refuses_modified_ancestry_limits_input_and_scope() {
    let (store, c, contexts, parent) = fixture().await;
    let child = capture_child(&contexts, &parent, "sealed-child", 'b').await;
    let reference = child.reference().unwrap();
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        reference.context_id().to_string(),
    );
    let original = store.get(&key).await.unwrap().unwrap();
    for mutation in ["root", "parent", "limits", "digest", "tenant", "depth"] {
        let mut envelope: serde_json::Value = serde_json::from_str(&original).unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_str(envelope["payload"].as_str().unwrap()).unwrap();
        match mutation {
            "root" => {
                payload["lineage"]["root_execution_id"] = uuid::Uuid::new_v4().to_string().into();
            }
            "parent" => {
                payload["lineage"]["parent"]["execution_id"] =
                    uuid::Uuid::new_v4().to_string().into();
            }
            "limits" => payload["lineage"]["limits"]["max_units"] = 2.into(),
            "digest" => payload["request_digest"] = "c".repeat(64).into(),
            "tenant" => payload["tenant"] = "foreign".into(),
            "depth" => payload["lineage"]["depth"] = 2.into(),
            _ => unreachable!(),
        }
        envelope["payload"] = payload.to_string().into();
        store.set(&key, &envelope.to_string(), None).await.unwrap();
        assert!(
            contexts.recover_reference(&reference, 100).await.is_err(),
            "{mutation}"
        );
        assert!(c.snapshot().await.unwrap().starts.is_empty());
    }
    store.set(&key, &original, None).await.unwrap();
    assert_eq!(
        contexts
            .recover_reference(&reference, 100)
            .await
            .unwrap()
            .root_execution_id(),
        parent.execution_id()
    );
}

#[tokio::test]
async fn parallel_signed_children_compete_for_one_remaining_root_unit() {
    let (state, c, contexts, parent) = fixture_with_limits(
        Arc::new(MemoryStateStore::new()),
        RootBudgetLimits {
            max_concurrent: 2,
            ..limits(2)
        },
    )
    .await;
    let StartRegistration::New(seed) = permitted(&c, &parent, "seed").await.unwrap() else {
        panic!("new seed")
    };
    c.settle("seed", &seed.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let left = capture_child(&contexts, &parent, "last-unit-left", 'b').await;
    let right = capture_child(&contexts, &parent, "last-unit-right", 'c').await;
    let peer = AuthorityCoordinator::connect(state, "city", "tenant")
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        permitted(&c, &left, "last-left"),
        permitted(&peer, &right, "last-right")
    );
    let (winner, record) = match (a, b) {
        (Ok(StartRegistration::New(r)), Err(CoordinationError::BudgetExhausted)) => {
            ("last-left", r)
        }
        (Err(CoordinationError::BudgetExhausted), Ok(StartRegistration::New(r))) => {
            ("last-right", r)
        }
        result => panic!("one unit must produce exactly one winner: {result:?}"),
    };
    peer.settle(winner, &record.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let snapshot = c.snapshot().await.unwrap();
    assert_eq!(
        snapshot.roots[&parent.execution_id().to_string()].spent_units,
        2
    );
    assert_eq!(
        snapshot.roots[&left.execution_id().to_string()].spent_units
            + snapshot.roots[&right.execution_id().to_string()].spent_units,
        1
    );
    assert!(snapshot.roots.values().all(|r| r.active_attempts == 0));
    assert_eq!(snapshot.starts.len(), 2);
}

#[tokio::test]
async fn execution_fence_blocks_descendants_but_allows_existing_settlement() {
    let (state, c, _, parent) = fixture().await;
    let left = child(&c, &parent, 2).await;
    let right = child(&c, &parent, 2).await;
    let StartRegistration::New(first) = start(&c, "before-stop", left).await.unwrap() else {
        panic!("expected new start")
    };
    c.settle("before-stop", &first.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    let root_id = parent.execution_id().to_string();
    c.change(
        "stop",
        acteon_governance::AuthorityChange::CancelExecution {
            execution_id: root_id.clone(),
        },
        "work-host",
        "cancel instance",
    )
    .await
    .unwrap();
    let peer = AuthorityCoordinator::connect(state, "city", "tenant")
        .await
        .unwrap();
    assert!(matches!(
        start(&peer, "after-stop", right).await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        peer.snapshot().await.unwrap().roots[&root_id].active_attempts,
        1
    );
    // Replay observes the original record; it cannot authorize another call.
    assert!(matches!(
        start(&peer, "before-stop", left).await,
        Ok(StartRegistration::Existing(_))
    ));
    peer.settle("before-stop", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    peer.settle("before-stop", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    let snapshot = peer.snapshot().await.unwrap();
    assert_eq!(
        (
            snapshot.roots[&root_id].spent_units,
            snapshot.roots[&root_id].active_attempts
        ),
        (1, 0)
    );
    assert!(snapshot.roots[&root_id].cancelled);
}
