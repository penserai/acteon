use acteon_core::{
    AgentOwnership, PrincipalIdentity, PrincipalKind, RepresentedParty, ResourceKind, ResourceRef,
    TeamRef, TeamRole, WorkforceAssignment, WorkforceMembership, WorkforceReference, WorkforceTeam,
};
use acteon_governance::context::{
    AcceptedEffect, ContextBinding, ContextSigningKey, ExecutionContextHandle,
    RootContextAdmission, TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::permit::{ExecutionPermit, PermitReference, PermittedAttempt};
use acteon_governance::workforce::{
    RepresentationMandate, WorkforceDependency, WorkforceJobAdmission,
    WorkforceManagementAuthorization, WorkforceManagementCeiling, WorkforceMutation,
};
use acteon_governance::{
    AttemptStatus, AuthorityCoordinator, CoordinationError, CoordinatorLimits, RootBudgetLimits,
    ScopePurpose, StartRegistration,
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use std::sync::Arc;

fn principal(id: &str, kind: PrincipalKind) -> PrincipalIdentity {
    PrincipalIdentity::new(id, kind).unwrap()
}
fn maya() -> PrincipalIdentity {
    principal("maya", PrincipalKind::Human)
}
fn operator() -> PrincipalIdentity {
    principal("operator", PrincipalKind::Human)
}
fn personal() -> PrincipalIdentity {
    principal("maya-assistant", PrincipalKind::Agent)
}
fn shared() -> PrincipalIdentity {
    principal("team-investigator", PrincipalKind::Agent)
}
fn team(id: &str) -> TeamRef {
    TeamRef::new("workforce", "acme", id).unwrap()
}
fn reference(id: &str) -> WorkforceReference {
    WorkforceReference {
        id: id.into(),
        accepted_revision: 1,
    }
}
fn effects() -> Vec<AcceptedEffect> {
    vec![AcceptedEffect {
        operation: "provider.execute".into(),
        resources: vec![
            ResourceRef::new(ResourceKind::Endpoint, "prod", "acme", "incident").unwrap(),
        ],
    }]
}
fn clock() -> acteon_time::ManualClock {
    acteon_time::ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
}
fn limits() -> RootBudgetLimits {
    RootBudgetLimits {
        max_units: 4,
        max_concurrent: 1,
        deadline_ms: 1500,
    }
}
fn ceiling() -> WorkforceManagementCeiling {
    WorkforceManagementCeiling {
        actor: operator(),
        teams: vec![team("reliability"), team("release")],
        principals: vec![maya(), personal(), shared(), operator()],
        job_classes: vec!["diagnose".into()],
        effects: effects(),
        valid_from_ms: 0,
        limits: RootBudgetLimits {
            max_units: 20,
            max_concurrent: 2,
            deadline_ms: 2000,
        },
        can_manage_roster: true,
        can_issue_mandates: true,
        can_issue_permits: true,
    }
}
async fn change(
    c: &AuthorityCoordinator,
    id: &str,
    mutation: WorkforceMutation,
) -> Result<(), CoordinationError> {
    c.change_workforce(
        id,
        mutation,
        "reviewed",
        WorkforceManagementAuthorization {
            ceiling: &ceiling(),
            evaluated_authority: &c.snapshot().await?.stamp(),
            clock: &clock(),
        },
    )
    .await
    .map(|_| ())
}
async fn fixture(store: Arc<dyn StateStore>) -> AuthorityCoordinator {
    let c = AuthorityCoordinator::initialize(store, "prod", "acme", CoordinatorLimits::default())
        .await
        .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    change(
        &c,
        "team",
        WorkforceMutation::PutTeam {
            team: WorkforceTeam {
                team: team("reliability"),
                revision: 1,
                name: "Reliability".into(),
            },
        },
    )
    .await
    .unwrap();
    change(
        &c,
        "membership",
        WorkforceMutation::PutMembership {
            membership: WorkforceMembership {
                id: "maya-reliability".into(),
                revision: 1,
                team: team("reliability"),
                human: maya(),
                roles: vec![TeamRole::Requester],
                valid_from_ms: 0,
                deadline_ms: 2000,
            },
        },
    )
    .await
    .unwrap();
    for (id, agent, owner) in [
        (
            "personal-owner",
            personal(),
            RepresentedParty::Human { principal: maya() },
        ),
        (
            "team-owner",
            shared(),
            RepresentedParty::Team {
                team: team("reliability"),
            },
        ),
    ] {
        change(
            &c,
            id,
            WorkforceMutation::PutOwnership {
                ownership: AgentOwnership {
                    agent,
                    revision: 1,
                    owner,
                },
            },
        )
        .await
        .unwrap();
    }
    change(
        &c,
        "assignment",
        WorkforceMutation::PutAssignment {
            assignment: WorkforceAssignment {
                id: "assistant-roster".into(),
                revision: 1,
                team: team("reliability"),
                agent: personal(),
                job_classes: vec!["diagnose".into()],
                valid_from_ms: 0,
                deadline_ms: 2000,
            },
        },
    )
    .await
    .unwrap();
    c
}
fn mandate(personal_agent: bool) -> RepresentationMandate {
    RepresentationMandate {
        id: if personal_agent {
            "personal-team"
        } else {
            "standing-team"
        }
        .into(),
        revision: 1,
        represented: RepresentedParty::Team {
            team: team("reliability"),
        },
        actor: if personal_agent { personal() } else { shared() },
        job_class: "diagnose".into(),
        eligible_initiators: vec![if personal_agent { personal() } else { shared() }],
        ownership: Some(reference(if personal_agent {
            "maya-assistant"
        } else {
            "team-investigator"
        })),
        dependencies: if personal_agent {
            vec![
                WorkforceDependency::Membership {
                    reference: reference("maya-reliability"),
                },
                WorkforceDependency::Assignment {
                    reference: reference("assistant-roster"),
                },
            ]
        } else {
            vec![]
        },
        effects: effects(),
        valid_from_ms: 0,
        limits: limits(),
    }
}
async fn issue(c: &AuthorityCoordinator, personal_agent: bool) -> Vec<PermitReference> {
    let policy = mandate(personal_agent);
    let id = policy.id.clone();
    change(
        c,
        &format!("mandate-{id}"),
        WorkforceMutation::PutMandate {
            mandate: policy.clone(),
        },
    )
    .await
    .unwrap();
    change(
        c,
        &format!("permit-{id}"),
        WorkforceMutation::PublishRepresentedPermit {
            permit: ExecutionPermit {
                id: id.clone(),
                revision: 1,
                subject: policy.actor,
                effects: effects(),
                valid_from_ms: 0,
                limits: limits(),
            },
            mandate: reference(&id),
        },
    )
    .await
    .unwrap();
    vec![PermitReference {
        id,
        accepted_revision: 1,
    }]
}
fn context_store(store: Arc<dyn StateStore>, c: AuthorityCoordinator) -> TrustedContextStore {
    TrustedContextStore::new(
        store,
        c,
        "domain".into(),
        "key".into(),
        vec![ContextSigningKey::new("key".into(), vec![9; 32]).unwrap()],
    )
    .unwrap()
}
async fn admit(
    c: &AuthorityCoordinator,
    store: Arc<dyn StateStore>,
    permits: &[PermitReference],
    actor: PrincipalIdentity,
) -> VerifiedExecutionContext {
    let admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor.clone(),
            request_digest: "a".repeat(64),
        },
        credential_id: "authenticated-credential".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "host-evaluated".into(),
        accepted_effects: effects(),
        deadline_ms: limits().deadline_ms,
        evaluated_authority: c.snapshot().await.unwrap().stamp(),
    };
    let contexts = context_store(store, c.clone());
    assert!(
        contexts
            .capture_permitted_root(admission.clone(), permits, limits(), &clock())
            .await
            .is_err(),
        "ordinary capture must not discard representation"
    );
    let proof = c
        .evaluate_representation(WorkforceJobAdmission {
            mandate: &reference(&permits[0].id),
            initiator: &actor,
            job_class: "diagnose",
            admission: &admission,
            limits: &limits(),
            clock: &clock(),
        })
        .await
        .unwrap();
    let context = contexts
        .capture_represented_permitted_root(admission, permits, limits(), &clock(), &proof)
        .await
        .unwrap();
    assert_eq!(context.principal(), &actor);
    assert_eq!(
        context.representation().unwrap().represented,
        RepresentedParty::Team {
            team: team("reliability")
        }
    );
    context
}
async fn start(
    c: &AuthorityCoordinator,
    context: &VerifiedExecutionContext,
    permits: &[PermitReference],
    id: &str,
) -> Result<StartRegistration, CoordinationError> {
    c.register_permitted_attempt(PermittedAttempt {
        id,
        context,
        permits,
        effect: &effects()[0],
        request_digest: &"a".repeat(64),
        units: 1,
        clock: &clock(),
    })
    .await
}

#[tokio::test]
async fn membership_offboarding_denies_personal_team_work_but_preserves_independent_standing_work()
{
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let personal_permits = issue(&c, true).await;
    let standing_permits = issue(&c, false).await;
    let personal_context = admit(&c, store.clone(), &personal_permits, personal()).await;
    let standing_context = admit(&c, store.clone(), &standing_permits, shared()).await;
    let first = start(&c, &personal_context, &personal_permits, "before-removal")
        .await
        .unwrap();
    let StartRegistration::New(first) = first else {
        panic!("fresh effect required")
    };
    c.settle("before-removal", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    change(
        &c,
        "offboard",
        WorkforceMutation::RemoveMembership {
            id: "maya-reliability".into(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        start(&c, &personal_context, &personal_permits, "after-removal").await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        start(
            &c,
            &standing_context,
            &standing_permits,
            "independent-standing"
        )
        .await,
        Ok(StartRegistration::New(_))
    ));
    let snapshot = c.snapshot().await.unwrap();
    assert!(!snapshot.starts.contains_key("after-removal"));
    assert!(snapshot.workforce.memberships["maya-reliability"].revoked);
    assert_eq!(
        snapshot.roots[&personal_context.execution_id().to_string()].spent_units,
        1
    );
    let peer = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    assert!(peer.snapshot().await.unwrap().workforce.memberships["maya-reliability"].revoked);
}

#[tokio::test]
async fn ownership_and_roster_do_not_issue_permits_or_allow_a_membership_union() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store).await;
    assert!(c.snapshot().await.unwrap().permits.is_empty());
    let mut wrong = mandate(true);
    wrong.represented = RepresentedParty::Team {
        team: team("release"),
    };
    change(
        &c,
        "release-team",
        WorkforceMutation::PutTeam {
            team: WorkforceTeam {
                team: team("release"),
                revision: 1,
                name: "Release".into(),
            },
        },
    )
    .await
    .unwrap();
    change(
        &c,
        "release-membership",
        WorkforceMutation::PutMembership {
            membership: WorkforceMembership {
                id: "maya-release".into(),
                revision: 1,
                team: team("release"),
                human: maya(),
                roles: vec![TeamRole::Requester],
                valid_from_ms: 0,
                deadline_ms: 2000,
            },
        },
    )
    .await
    .unwrap();
    assert!(
        change(
            &c,
            "wrong-team-mandate",
            WorkforceMutation::PutMandate { mandate: wrong }
        )
        .await
        .is_err()
    );
    let mut missing = mandate(true);
    missing.dependencies.clear();
    assert!(
        change(
            &c,
            "unsponsored",
            WorkforceMutation::PutMandate { mandate: missing }
        )
        .await
        .is_err()
    );
    let mut roster_only = ceiling();
    roster_only.can_issue_mandates = false;
    roster_only.can_issue_permits = false;
    assert!(matches!(
        c.change_workforce(
            "forbidden-mandate",
            WorkforceMutation::PutMandate {
                mandate: mandate(true)
            },
            "reviewed",
            WorkforceManagementAuthorization {
                ceiling: &roster_only,
                evaluated_authority: &c.snapshot().await.unwrap().stamp(),
                clock: &clock()
            }
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn transfer_and_disband_never_retarget_an_admitted_job() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let permits = issue(&c, false).await;
    let ctx = admit(&c, store, &permits, shared()).await;
    change(
        &c,
        "transfer",
        WorkforceMutation::PutOwnership {
            ownership: AgentOwnership {
                agent: shared(),
                revision: 2,
                owner: RepresentedParty::Human { principal: maya() },
            },
        },
    )
    .await
    .unwrap();
    assert!(start(&c, &ctx, &permits, "transferred").await.is_err());
    assert_eq!(
        ctx.representation().unwrap().represented,
        RepresentedParty::Team {
            team: team("reliability")
        }
    );
    change(
        &c,
        "disband",
        WorkforceMutation::DisbandTeam {
            team: team("reliability"),
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    assert!(start(&c, &ctx, &permits, "disbanded").await.is_err());
    assert!(
        change(
            &c,
            "resurrect-team",
            WorkforceMutation::PutTeam {
                team: WorkforceTeam {
                    team: team("reliability"),
                    revision: 2,
                    name: "Reliability again".into()
                }
            }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn generic_publication_cannot_remove_a_required_mandate_from_a_represented_permit() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let permits = issue(&c, false).await;
    let mut updated = c.snapshot().await.unwrap().permits[&permits[0].id]
        .permit
        .clone();
    updated.revision = 2;
    let issuance = acteon_governance::permit::PermitIssuanceCeiling {
        issuer: operator(),
        subjects: vec![shared()],
        effects: effects(),
        valid_from_ms: 0,
        limits: limits(),
    };
    assert!(matches!(
        c.publish_permit(
            "discard-mandate",
            updated,
            1,
            &issuance,
            &c.snapshot().await.unwrap().stamp(),
            "reviewed",
            100
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        c.snapshot().await.unwrap().permits[&permits[0].id]
            .permit
            .revision,
        1
    );
}

#[tokio::test]
async fn lost_roster_ack_replay_is_observation_and_requires_current_manager_authority() {
    use acteon_state::KeyKind;
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let c = fixture(faults.clone()).await;
    let mutation = WorkforceMutation::RemoveMembership {
        id: "maya-reliability".into(),
        expected_revision: 1,
    };
    faults
        .fail_next(
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        change(&c, "lost-offboarding", mutation.clone())
            .await
            .is_err()
    );
    let committed = c.snapshot().await.unwrap();
    assert!(committed.workforce.memberships["maya-reliability"].revoked);
    change(&c, "lost-offboarding", mutation.clone())
        .await
        .unwrap();
    assert_eq!(c.snapshot().await.unwrap().generation, committed.generation);
    c.change(
        "manager-disabled",
        acteon_governance::AuthorityChange::RevokeSubject {
            subject: operator().id().into(),
        },
        "security",
        "offboard manager",
    )
    .await
    .unwrap();
    assert!(matches!(
        change(&c, "lost-offboarding", mutation).await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn workforce_management_resamples_expiry_after_same_generation_conflict() {
    use acteon_state::KeyKind;
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let raw: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(raw.clone()));
    let c = fixture(faults.clone()).await;
    let peer = AuthorityCoordinator::connect(raw, "prod", "acme")
        .await
        .unwrap();
    let stamp = c.snapshot().await.unwrap().stamp();
    let saved_stamp = stamp.clone();
    let time = clock();
    let task_time = time.clone();
    let release = faults
        .pause_next(
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let task = tokio::spawn(async move {
        c.change_workforce(
            "expired",
            WorkforceMutation::RemoveMembership {
                id: "maya-reliability".into(),
                expected_revision: 1,
            },
            "reviewed",
            WorkforceManagementAuthorization {
                ceiling: &ceiling(),
                evaluated_authority: &stamp,
                clock: &task_time,
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    peer.acknowledge_change("team").await.unwrap();
    assert_eq!(peer.snapshot().await.unwrap().stamp(), saved_stamp);
    time.advance_to(std::time::Duration::from_millis(2100))
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::Restricted)
    ));
    let state = peer.snapshot().await.unwrap();
    assert!(!state.workforce.memberships["maya-reliability"].revoked);
    assert!(!state.changes.contains_key("expired"));
}

#[tokio::test]
async fn membership_removal_wins_before_effect_registration_cas() {
    use acteon_state::KeyKind;
    use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
    let raw: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let faults = Arc::new(FaultStore::new(raw.clone()));
    let c = fixture(faults.clone()).await;
    let permits = issue(&c, true).await;
    let ctx = admit(&c, faults.clone(), &permits, personal()).await;
    let peer = AuthorityCoordinator::connect(raw, "prod", "acme")
        .await
        .unwrap();
    let root = ctx.execution_id().to_string();
    let release = faults
        .pause_next(
            KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let task = tokio::spawn(async move { start(&c, &ctx, &permits, "racing-effect").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    change(
        &peer,
        "racing-offboard",
        WorkforceMutation::RemoveMembership {
            id: "maya-reliability".into(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::StaleAuthority)
    ));
    let state = peer.snapshot().await.unwrap();
    assert!(!state.starts.contains_key("racing-effect"));
    assert_eq!(state.roots[&root].spent_units, 0);
}

#[tokio::test]
async fn claimed_protocol_eight_requires_reviewed_cutover_and_preserves_accounting_and_identity() {
    use acteon_state::{KeyKind, StateKey};
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = AuthorityCoordinator::initialize(
        store.clone(),
        "prod",
        "acme",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    c.reserve_scope(ScopePurpose::Execution).await.unwrap();
    let original = c.snapshot().await.unwrap();
    let mut source = serde_json::to_value(&original).unwrap();
    source["schema_version"] = 8.into();
    source.as_object_mut().unwrap().remove("workforce");
    source.as_object_mut().unwrap().remove("agent_registry");
    source.as_object_mut().unwrap().remove("budget_parents");
    let key = StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    store.set(&key, &source.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store.clone(), "prod", "acme")
            .await
            .is_err()
    );
    let plan = AuthorityCoordinator::plan_scope_upgrade(
        store.clone(),
        "prod",
        "acme",
        ScopePurpose::Execution,
        "operator",
        "reviewed workforce protocol",
    )
    .await
    .unwrap();
    assert_eq!(plan.report().from_protocol, 8);
    assert_eq!(plan.report().to_protocol, 11);
    let before = store.get_versioned(&key).await.unwrap();
    assert!(plan.apply("unreviewed").await.is_err());
    assert_eq!(store.get_versioned(&key).await.unwrap(), before);
    assert!(plan.apply(&plan.report().review_digest).await.unwrap());
    assert!(!plan.apply(&plan.report().review_digest).await.unwrap());
    let upgraded = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(upgraded.incarnation, original.incarnation);
    assert_eq!(upgraded.roots, original.roots);
    assert_eq!(upgraded.starts, original.starts);
    assert_eq!(upgraded.generation, original.generation + 1);
    assert_eq!(upgraded.purpose, ScopePurpose::Execution);
}

async fn backend_contract(a: Arc<dyn StateStore>, b: Arc<dyn StateStore>) {
    use acteon_state::{KeyKind, StateKey};
    let c = fixture(a.clone()).await;
    let permits = issue(&c, true).await;
    let ctx = admit(&c, a.clone(), &permits, personal()).await;
    let peer = AuthorityCoordinator::connect(b.clone(), "prod", "acme")
        .await
        .unwrap();
    let contexts = context_store(b.clone(), peer.clone());
    change(
        &c,
        "durable-offboard",
        WorkforceMutation::RemoveMembership {
            id: "maya-reliability".into(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    let recovered = contexts
        .recover_reference(&ctx.reference().unwrap(), 100)
        .await
        .unwrap();
    assert_eq!(recovered.representation(), ctx.representation());
    assert!(matches!(
        start(&peer, &recovered, &permits, "recovered-denied").await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        peer.snapshot().await.unwrap().roots[&ctx.execution_id().to_string()].spent_units,
        0
    );
    // These stores have unique test prefixes/tables; remove only our exact records.
    b.delete(&StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_governance::context::CONTEXT_KIND.into()),
        ctx.reference().unwrap().context_id().to_string(),
    ))
    .await
    .unwrap();
    b.delete(&StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL"]
async fn independent_redis_workforce_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("workforce-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    backend_contract(
        Arc::new(RedisStateStore::new(&config).unwrap()),
        Arc::new(RedisStateStore::new(&config).unwrap()),
    )
    .await;
}
#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn independent_postgres_workforce_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("workforce_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    backend_contract(
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
async fn permit_representation_requires_exact_job_initiator_and_bound_input() {
    use acteon_governance::workforce::WorkforcePermitAdmission;
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let permits = issue(&c, true).await;
    let actor = personal();
    let admission = RootContextAdmission {
        handle: ExecutionContextHandle::new(),
        binding: ContextBinding {
            execution_id: uuid::Uuid::new_v4(),
            principal: actor.clone(),
            request_digest: "a".repeat(64),
        },
        credential_id: "authenticated-credential".into(),
        auth_method: "api_key".into(),
        accepted_ceiling_revision: "host-evaluated".into(),
        accepted_effects: effects(),
        deadline_ms: limits().deadline_ms,
        evaluated_authority: c.snapshot().await.unwrap().stamp(),
    };
    for (initiator, job_class) in [(maya(), "diagnose"), (actor.clone(), "remediate")] {
        assert!(matches!(
            c.evaluate_permit_representation(WorkforcePermitAdmission {
                permits: &permits,
                initiator: &initiator,
                job_class,
                admission: &admission,
                limits: &limits(),
                clock: &clock(),
            })
            .await,
            Err(CoordinationError::Restricted)
        ));
    }
    let proof = c
        .evaluate_permit_representation(WorkforcePermitAdmission {
            permits: &permits,
            initiator: &actor,
            job_class: "diagnose",
            admission: &admission,
            limits: &limits(),
            clock: &clock(),
        })
        .await
        .unwrap()
        .unwrap();
    let contexts = context_store(store, c.clone());
    let mut replaced_input = admission.clone();
    replaced_input.binding.request_digest = "b".repeat(64);
    assert!(
        contexts
            .capture_represented_permitted_root(
                replaced_input,
                &permits,
                limits(),
                &clock(),
                &proof
            )
            .await
            .is_err()
    );
    assert!(c.snapshot().await.unwrap().roots.is_empty());
    let original = contexts
        .capture_represented_permitted_root(admission.clone(), &permits, limits(), &clock(), &proof)
        .await
        .unwrap();
    assert_eq!(original.representation().unwrap().job_class, "diagnose");
    let shared_permits = issue(&c, false).await;
    let mut mixed = permits;
    mixed.extend(shared_permits);
    let mut current = admission;
    current.evaluated_authority = c.snapshot().await.unwrap().stamp();
    assert!(matches!(
        c.evaluate_permit_representation(WorkforcePermitAdmission {
            permits: &mixed,
            initiator: &actor,
            job_class: "diagnose",
            admission: &current,
            limits: &limits(),
            clock: &clock(),
        })
        .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn management_cannot_assign_or_mandate_an_undeclared_job_class() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let mut bounds = ceiling();
    bounds.job_classes = vec!["remediate".into()];
    let before = c.snapshot().await.unwrap();
    for (id, mutation) in [
        (
            "forbidden-mandate",
            WorkforceMutation::PutMandate {
                mandate: mandate(true),
            },
        ),
        (
            "forbidden-assignment",
            WorkforceMutation::PutAssignment {
                assignment: WorkforceAssignment {
                    id: "other-assignment".into(),
                    revision: 1,
                    team: team("reliability"),
                    agent: personal(),
                    job_classes: vec!["diagnose".into()],
                    valid_from_ms: 0,
                    deadline_ms: 1500,
                },
            },
        ),
    ] {
        assert!(matches!(
            c.change_workforce(
                id,
                mutation,
                "reviewed",
                WorkforceManagementAuthorization {
                    ceiling: &bounds,
                    evaluated_authority: &before.stamp(),
                    clock: &clock(),
                }
            )
            .await,
            Err(CoordinationError::Restricted)
        ));
    }
    assert_eq!(c.snapshot().await.unwrap().stamp(), before.stamp());
}

#[tokio::test]
async fn revoking_a_human_blocks_explicitly_dependent_agents_but_not_standing_team_work() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let personal_permits = issue(&c, true).await;
    let shared_permits = issue(&c, false).await;
    let assistant = admit(&c, store.clone(), &personal_permits, personal()).await;
    let standing = admit(&c, store, &shared_permits, shared()).await;
    c.change(
        "revoke-maya",
        acteon_governance::AuthorityChange::RevokeSubject {
            subject: maya().id().into(),
        },
        "security",
        "offboard human",
    )
    .await
    .unwrap();
    assert!(matches!(
        start(&c, &assistant, &personal_permits, "dependent-effect").await,
        Err(CoordinationError::Restricted)
    ));
    assert!(
        start(&c, &standing, &shared_permits, "standing-effect")
            .await
            .is_ok()
    );
    assert_eq!(
        c.snapshot().await.unwrap().roots[&assistant.execution_id().to_string()].spent_units,
        0
    );
}

#[tokio::test]
async fn represented_permits_cannot_be_issued_after_actor_revocation() {
    let c = fixture(Arc::new(MemoryStateStore::new())).await;
    let selected = issue(&c, false).await;
    c.change(
        "offboard-investigator",
        acteon_governance::AuthorityChange::RevokeSubject {
            subject: shared().id().into(),
        },
        "security",
        "offboarding",
    )
    .await
    .unwrap();
    let before = c.snapshot().await.unwrap();
    let mut permit = before.permits[&selected[0].id].permit.clone();
    permit.id = "new-standing-permit".into();
    assert!(matches!(
        change(
            &c,
            "issue-after-offboarding",
            WorkforceMutation::PublishRepresentedPermit {
                permit,
                mandate: reference("standing-team"),
            }
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    let after = c.snapshot().await.unwrap();
    assert_eq!(after.stamp(), before.stamp());
    assert_eq!(after.permits, before.permits);
    assert_eq!(
        after.workforce.permit_bindings,
        before.workforce.permit_bindings
    );
}

#[tokio::test]
async fn descendants_preserve_representation_and_current_membership_dependencies() {
    use acteon_governance::context::ChildContextAdmission;
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let permits = issue(&c, true).await;
    let parent = admit(&c, store.clone(), &permits, personal()).await;
    let contexts = context_store(store, c.clone());
    let child = contexts
        .capture_child(ChildContextAdmission {
            admission_key: "represented-child",
            parent: &parent,
            handle: ExecutionContextHandle::new(),
            execution_id: uuid::Uuid::new_v4(),
            request_digest: "b".repeat(64),
            accepted_effects: effects(),
            restrictions: vec![],
            permits: &permits,
            limits: RootBudgetLimits {
                max_units: 2,
                ..limits()
            },
            clock: &clock(),
        })
        .await
        .unwrap();
    assert_eq!(child.principal(), parent.principal());
    assert_eq!(child.representation(), parent.representation());
    assert_eq!(
        parent.original_requester(),
        &parent.representation().unwrap().initiator
    );
    assert_eq!(
        child.original_requester(),
        &parent.representation().unwrap().initiator
    );
    let grandchild = contexts
        .capture_child(ChildContextAdmission {
            admission_key: "represented-grandchild",
            parent: &child,
            handle: ExecutionContextHandle::new(),
            execution_id: uuid::Uuid::new_v4(),
            request_digest: "a".repeat(64),
            accepted_effects: effects(),
            restrictions: vec![],
            permits: &permits,
            limits: RootBudgetLimits {
                max_units: 1,
                ..limits()
            },
            clock: &clock(),
        })
        .await
        .unwrap();
    assert_eq!(grandchild.root_execution_id(), parent.execution_id());
    assert_eq!(
        grandchild.original_requester(),
        &parent.representation().unwrap().initiator
    );
    change(
        &c,
        "offboard-descendant",
        WorkforceMutation::RemoveMembership {
            id: "maya-reliability".into(),
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    assert!(
        start(&c, &grandchild, &permits, "revoked-grandchild")
            .await
            .is_err()
    );
    assert!(
        contexts
            .capture_child(ChildContextAdmission {
                admission_key: "after-offboard",
                parent: &child,
                handle: ExecutionContextHandle::new(),
                execution_id: uuid::Uuid::new_v4(),
                request_digest: "c".repeat(64),
                accepted_effects: effects(),
                restrictions: vec![],
                permits: &permits,
                limits: RootBudgetLimits {
                    max_units: 1,
                    ..limits()
                },
                clock: &clock(),
            })
            .await
            .is_err()
    );
    let snapshot = c.snapshot().await.unwrap();
    assert!(snapshot.starts.is_empty());
    assert!(snapshot.roots.values().all(|r| r.spent_units == 0));
}

#[tokio::test]
async fn protocol_nine_cutover_preserves_workforce_and_live_accounting() {
    use acteon_state::{KeyKind, StateKey};
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let c = fixture(store.clone()).await;
    let permits = issue(&c, true).await;
    let parent = admit(&c, store.clone(), &permits, personal()).await;
    let StartRegistration::New(first) = start(&c, &parent, &permits, "live-at-cutover")
        .await
        .unwrap()
    else {
        panic!("new start")
    };
    c.settle("live-at-cutover", &first.token, AttemptStatus::Uncertain)
        .await
        .unwrap();
    // Retain an earlier reviewed 7→9 cutover while adding the new 9→10 event.
    let mut historical = serde_json::to_value(c.snapshot().await.unwrap()).unwrap();
    let generation = historical["generation"].as_u64().unwrap() + 1;
    historical["generation"] = generation.into();
    historical["changes"]["scope-protocol-9"] = serde_json::json!({
        "change": {"kind":"upgrade_protocol", "from_protocol":7, "to_protocol":9},
        "actor":"operator", "reason":"previous reviewed cutover",
        "generation": generation, "pending":false,
    });
    let historical_key = StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    store
        .set(&historical_key, &historical.to_string(), None)
        .await
        .unwrap();
    let original = c.snapshot().await.unwrap();
    let mut source = serde_json::to_value(&original).unwrap();
    source["schema_version"] = 9.into();
    source.as_object_mut().unwrap().remove("budget_parents");
    source.as_object_mut().unwrap().remove("agent_registry");
    let key = StateKey::new(
        "prod",
        "acme",
        KeyKind::Custom(acteon_governance::COORDINATOR_KIND.into()),
        "authority",
    );
    store.set(&key, &source.to_string(), None).await.unwrap();
    assert!(
        AuthorityCoordinator::connect(store.clone(), "prod", "acme")
            .await
            .is_err()
    );
    let plan = AuthorityCoordinator::plan_scope_upgrade(
        store.clone(),
        "prod",
        "acme",
        ScopePurpose::Execution,
        "operator",
        "reviewed descendant accounting cutover",
    )
    .await
    .unwrap();
    assert_eq!(plan.report().from_protocol, 9);
    assert_eq!(plan.report().to_protocol, 11);
    assert_eq!(plan.report().unsettled_starts, 1);
    assert!(plan.apply(&plan.report().review_digest).await.unwrap());
    let recovered = AuthorityCoordinator::connect(store, "prod", "acme")
        .await
        .unwrap();
    let migrated = recovered.snapshot().await.unwrap();
    assert_eq!(
        serde_json::to_value(&migrated.workforce).unwrap(),
        serde_json::to_value(&original.workforce).unwrap()
    );
    assert_eq!(migrated.roots, original.roots);
    assert_eq!(migrated.starts, original.starts);
    assert_eq!(migrated.permits, original.permits);
    assert_eq!(migrated.incarnation, original.incarnation);
    assert!(migrated.budget_parents.is_empty());
    recovered
        .settle("live-at-cutover", &first.token, AttemptStatus::Settled)
        .await
        .unwrap();
    assert_eq!(
        recovered.snapshot().await.unwrap().roots[&parent.execution_id().to_string()]
            .active_attempts,
        0
    );
}
