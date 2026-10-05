use std::{sync::Arc, time::Duration};

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, COORDINATOR_KIND, CoordinationError,
    CoordinatorLimits, RootBudgetLimits,
    control::{ControlChangeAuthorization, ControlChangeCeiling},
    permit::{ExecutionPermit, PermitIssuanceCeiling},
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;

fn resource(id: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", id).unwrap()
}
fn actor(id: &str) -> PrincipalIdentity {
    PrincipalIdentity::new(id, PrincipalKind::Human).unwrap()
}
fn ceiling() -> ControlChangeCeiling {
    ControlChangeCeiling {
        actor: actor("operator"),
        subjects: vec![actor("participant")],
        resources: vec![resource("building"), resource("utility")],
        valid_from_ms: 0,
        deadline_ms: 1000,
    }
}
fn clock() -> ManualClock {
    ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap())
}
fn authorization<'a>(
    ceiling: &'a ControlChangeCeiling,
    stamp: &'a AuthorityStamp,
    clock: &'a ManualClock,
) -> ControlChangeAuthorization<'a> {
    ControlChangeAuthorization {
        ceiling,
        evaluated_authority: stamp,
        clock,
    }
}
fn close() -> AuthorityChange {
    AuthorityChange::CloseResource {
        resource: resource("building"),
    }
}
async fn coordinator(store: Arc<dyn StateStore>) -> AuthorityCoordinator {
    let coordinator =
        AuthorityCoordinator::initialize(store, "city", "tenant", CoordinatorLimits::default())
            .await
            .unwrap();
    coordinator
        .reserve_scope(acteon_governance::ScopePurpose::Execution)
        .await
        .unwrap();
    coordinator
}
async fn wait_for_fault(faults: &FaultStore) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("control request did not reach CAS barrier");
}

async fn contract(store: Arc<dyn StateStore>, peer: Arc<dyn StateStore>) {
    let faults = Arc::new(FaultStore::new(store));
    let a = coordinator(faults.clone()).await;
    let b = AuthorityCoordinator::connect(peer, "city", "tenant")
        .await
        .unwrap();
    let policy = ceiling();
    let time = clock();
    let stamp = a.snapshot().await.unwrap().stamp();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        a.change_evaluated(
            "close",
            close(),
            "maintenance",
            authorization(&policy, &stamp, &time)
        )
        .await
        .is_err()
    );
    let snapshot = b.snapshot().await.unwrap();
    assert!(snapshot.closed_resources.contains(&resource("building")));
    assert!(snapshot.changes["close"].pending);
    let current = snapshot.stamp();
    assert!(matches!(
        a.change_evaluated(
            "close",
            close(),
            "maintenance",
            authorization(&policy, &stamp, &time)
        )
        .await,
        Err(CoordinationError::StaleAuthority)
    ));
    let record = a
        .change_evaluated(
            "close",
            close(),
            "maintenance",
            authorization(&policy, &current, &time),
        )
        .await
        .unwrap();
    assert_eq!(record.actor, "operator");
    assert_eq!(record, snapshot.changes["close"]);
    assert_eq!(b.snapshot().await.unwrap().generation, current.generation);
    assert!(matches!(
        a.change_evaluated(
            "close",
            close(),
            "different",
            authorization(&policy, &current, &time)
        )
        .await,
        Err(CoordinationError::Conflict)
    ));
    let record = b
        .change_evaluated(
            "reopen",
            AuthorityChange::ReopenResource {
                resource: resource("building"),
            },
            "complete",
            authorization(&policy, &current, &time),
        )
        .await
        .unwrap();
    let reopened = a.snapshot().await.unwrap();
    assert!(!reopened.closed_resources.contains(&resource("building")));
    assert_eq!(reopened.generation, record.generation);
    a.change_evaluated(
        "close",
        close(),
        "maintenance",
        authorization(&policy, &reopened.stamp(), &time),
    )
    .await
    .unwrap();
    assert!(
        !b.snapshot()
            .await
            .unwrap()
            .closed_resources
            .contains(&resource("building"))
    );
    b.change(
        "revoke-operator",
        AuthorityChange::RevokeSubject {
            subject: "operator".into(),
        },
        "security",
        "compromised",
    )
    .await
    .unwrap();
    let revoked = a.snapshot().await.unwrap().stamp();
    assert!(matches!(
        a.change_evaluated(
            "close",
            close(),
            "maintenance",
            authorization(&policy, &revoked, &time)
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn lost_ack_replay_requires_current_authority_and_never_recloses() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    contract(store.clone(), store).await;
}

async fn contention_contract(store: Arc<dyn StateStore>, peer: Arc<dyn StateStore>, expire: bool) {
    let faults = Arc::new(FaultStore::new(store));
    let a = coordinator(faults.clone()).await;
    let b = AuthorityCoordinator::connect(peer, "city", "tenant")
        .await
        .unwrap();
    if expire {
        b.change(
            "existing-event",
            AuthorityChange::CloseResource {
                resource: resource("utility"),
            },
            "security",
            "utility maintenance",
        )
        .await
        .unwrap();
    }
    let release = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let time = clock();
    let task_time = time.clone();
    let stamp = a.snapshot().await.unwrap().stamp();
    let task = tokio::spawn(async move {
        a.change_evaluated(
            "racing-close",
            close(),
            "maintenance",
            authorization(&ceiling(), &stamp, &task_time),
        )
        .await
    });
    wait_for_fault(&faults).await;
    if expire {
        // Outbox acknowledgment changes the CAS version without changing the
        // authority stamp. The retry must resample time, not reuse the first
        // successful validity observation.
        b.acknowledge_change("existing-event").await.unwrap();
        time.advance_to(Duration::from_millis(900)).unwrap();
    } else {
        b.change(
            "concurrent",
            AuthorityChange::RevokeSubject {
                subject: "operator".into(),
            },
            "security",
            "operator removed",
        )
        .await
        .unwrap();
    }
    release.send(()).unwrap();
    let result = task.await.unwrap();
    if expire {
        assert!(matches!(result, Err(CoordinationError::Restricted)));
    } else {
        assert!(matches!(result, Err(CoordinationError::StaleAuthority)));
    }
    let snapshot = b.snapshot().await.unwrap();
    assert!(!snapshot.changes.contains_key("racing-close"));
    assert!(!snapshot.closed_resources.contains(&resource("building")));
}

#[tokio::test]
async fn revocation_wins_before_control_cas() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    contention_contract(store.clone(), store, false).await;
}

#[tokio::test]
async fn expiry_is_resampled_after_a_cas_conflict_with_unchanged_authority() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    contention_contract(store.clone(), store, true).await;
}

#[tokio::test]
async fn independent_resource_subject_and_time_bounds_are_required() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    let stamp = c.snapshot().await.unwrap().stamp();
    let policy = ceiling();
    let time = clock();
    for change in [
        AuthorityChange::CloseResource {
            resource: resource("foreign"),
        },
        AuthorityChange::ReopenResource {
            resource: resource("foreign"),
        },
        AuthorityChange::RevokeSubject {
            subject: "foreign".into(),
        },
        AuthorityChange::RevokePermit {
            permit_id: "missing".into(),
            expected_revision: 1,
        },
        AuthorityChange::RevokeCredential {
            credential_id: "missing".into(),
            expected_revision: 1,
        },
    ] {
        assert!(matches!(
            c.change_evaluated(
                "denied",
                change,
                "reviewed",
                authorization(&policy, &stamp, &time)
            )
            .await,
            Err(CoordinationError::Restricted)
        ));
    }
    let mut foreign_scope = ceiling();
    foreign_scope
        .resources
        .push(ResourceRef::new(ResourceKind::Endpoint, "city", "other", "building").unwrap());
    assert!(
        c.change_evaluated(
            "foreign-scope",
            close(),
            "reviewed",
            authorization(&foreign_scope, &stamp, &time)
        )
        .await
        .is_err()
    );
    time.advance_to(Duration::from_millis(900)).unwrap();
    assert!(matches!(
        c.change_evaluated(
            "expired",
            close(),
            "reviewed",
            authorization(&policy, &stamp, &time)
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(c.snapshot().await.unwrap().changes.len(), 1);
}

#[tokio::test]
async fn revocation_requires_the_entire_current_policy_not_just_one_resource() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    let policy = ceiling();
    let permit = ExecutionPermit {
        id: "participant-permit".into(),
        revision: 1,
        subject: actor("participant"),
        effects: vec![acteon_governance::context::AcceptedEffect {
            operation: "work".into(),
            resources: policy.resources.clone(),
        }],
        valid_from_ms: 0,
        limits: RootBudgetLimits {
            max_units: 10,
            max_concurrent: 1,
            deadline_ms: 900,
        },
    };
    let issuance = PermitIssuanceCeiling {
        issuer: actor("publisher"),
        subjects: policy.subjects.clone(),
        effects: permit.effects.clone(),
        valid_from_ms: 0,
        limits: permit.limits.clone(),
    };
    c.publish_permit(
        "issue",
        permit,
        0,
        &issuance,
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        100,
    )
    .await
    .unwrap();
    let stamp = c.snapshot().await.unwrap().stamp();
    let time = clock();
    let revoke = AuthorityChange::RevokePermit {
        permit_id: "participant-permit".into(),
        expected_revision: 1,
    };
    let mut partial = ceiling();
    partial.resources.pop();
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            revoke.clone(),
            "reviewed",
            authorization(&partial, &stamp, &time)
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    let mut wrong_subject = ceiling();
    wrong_subject.subjects = vec![actor("other")];
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            revoke.clone(),
            "reviewed",
            authorization(&wrong_subject, &stamp, &time)
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        c.change_evaluated(
            "wrong-revision",
            AuthorityChange::RevokePermit {
                permit_id: "participant-permit".into(),
                expected_revision: 2
            },
            "reviewed",
            authorization(&policy, &stamp, &time)
        )
        .await,
        Err(CoordinationError::Conflict)
    ));
    // The target permit has expired, but current management authority can
    // still revoke it. Intervention validity is independent of target validity.
    time.advance_to(Duration::from_millis(800)).unwrap();
    c.change_evaluated(
        "revoke",
        revoke,
        "reviewed",
        authorization(&policy, &stamp, &time),
    )
    .await
    .unwrap();
    assert!(c.snapshot().await.unwrap().permits["participant-permit"].revoked);
}

#[test]
fn malformed_control_ceilings_are_not_authority() {
    for invalid in 0..5 {
        let mut policy = ceiling();
        match invalid {
            0 => {
                policy.subjects.clear();
                policy.resources.clear();
            }
            1 => policy.subjects.push(actor("participant")),
            2 => policy.resources.push(resource("building")),
            3 => policy.valid_from_ms = -1,
            _ => policy.deadline_ms = policy.valid_from_ms,
        }
        assert!(policy.validate().is_err());
    }
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL"]
async fn independent_redis_control_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("evaluated-control-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let b: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    contract(a.clone(), b.clone()).await;
    a.delete(&key).await.unwrap();
    contention_contract(a.clone(), b.clone(), false).await;
    a.delete(&key).await.unwrap();
    contention_contract(a.clone(), b, true).await;
    a.delete(&key).await.unwrap();
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn independent_postgres_control_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("control_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let a: Arc<dyn StateStore> = Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let b: Arc<dyn StateStore> = Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let key = StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    contract(a.clone(), b.clone()).await;
    a.delete(&key).await.unwrap();
    contention_contract(a.clone(), b.clone(), false).await;
    a.delete(&key).await.unwrap();
    contention_contract(a.clone(), b, true).await;
    a.delete(&key).await.unwrap();
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
async fn execution_management_cannot_mutate_authentication_control_or_unclaimed_scopes() {
    for purpose in [
        acteon_governance::ScopePurpose::Unclaimed,
        acteon_governance::ScopePurpose::AuthenticationControl {
            source_id: "auth".into(),
        },
    ] {
        let c = AuthorityCoordinator::initialize(
            Arc::new(MemoryStateStore::new()),
            "city",
            "tenant",
            CoordinatorLimits::default(),
        )
        .await
        .unwrap();
        if purpose != acteon_governance::ScopePurpose::Unclaimed {
            c.reserve_scope(purpose).await.unwrap();
        }
        let snapshot = c.snapshot().await.unwrap();
        assert!(matches!(
            c.change_evaluated(
                "revoke",
                AuthorityChange::RevokeSubject {
                    subject: "participant".into()
                },
                "reviewed",
                authorization(&ceiling(), &snapshot.stamp(), &clock())
            )
            .await,
            Err(CoordinationError::Restricted)
        ));
        assert_eq!(c.snapshot().await.unwrap().generation, snapshot.generation);
    }
}

#[tokio::test]
async fn credential_revocation_requires_typed_subject_and_complete_resources() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    let policy = ceiling();
    let credential = acteon_governance::credential::CredentialAuthority {
        ceiling: ExecutionPermit {
            id: "credential".into(),
            revision: 1,
            subject: actor("participant"),
            effects: vec![acteon_governance::context::AcceptedEffect {
                operation: "work".into(),
                resources: policy.resources.clone(),
            }],
            valid_from_ms: 0,
            limits: RootBudgetLimits {
                max_units: 10,
                max_concurrent: 1,
                deadline_ms: 900,
            },
        },
        auth_method: "api_key".into(),
        execution_enabled: true,
    };
    let issuance = PermitIssuanceCeiling {
        issuer: actor("publisher"),
        subjects: policy.subjects.clone(),
        effects: credential.ceiling.effects.clone(),
        valid_from_ms: 0,
        limits: credential.ceiling.limits.clone(),
    };
    c.publish_credential(
        "enroll",
        credential,
        0,
        &issuance,
        &c.snapshot().await.unwrap().stamp(),
        "reviewed",
        100,
    )
    .await
    .unwrap();
    let stamp = c.snapshot().await.unwrap().stamp();
    let revoke = AuthorityChange::RevokeCredential {
        credential_id: "credential".into(),
        expected_revision: 1,
    };
    let mut partial = ceiling();
    partial.resources.pop();
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            revoke.clone(),
            "reviewed",
            authorization(&partial, &stamp, &clock())
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    let mut wrong_kind = ceiling();
    wrong_kind.subjects =
        vec![PrincipalIdentity::new("participant", PrincipalKind::Agent).unwrap()];
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            revoke.clone(),
            "reviewed",
            authorization(&wrong_kind, &stamp, &clock())
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    c.change_evaluated(
        "revoke",
        revoke,
        "reviewed",
        authorization(&policy, &stamp, &clock()),
    )
    .await
    .unwrap();
    assert!(c.snapshot().await.unwrap().credentials["credential"].revoked);
}

#[tokio::test]
async fn future_authority_and_foreign_incarnation_cannot_revoke_a_subject() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    let stamp = c.snapshot().await.unwrap().stamp();
    let mut policy = ceiling();
    let time = clock();
    let change = AuthorityChange::RevokeSubject {
        subject: "participant".into(),
    };
    policy.valid_from_ms = 101;
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            change.clone(),
            "reviewed",
            authorization(&policy, &stamp, &time)
        )
        .await,
        Err(CoordinationError::Restricted)
    ));
    policy.valid_from_ms = 0;
    let mut foreign = stamp.clone();
    foreign.incarnation = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        c.change_evaluated(
            "revoke",
            change.clone(),
            "reviewed",
            authorization(&policy, &foreign, &time)
        )
        .await,
        Err(CoordinationError::StaleAuthority)
    ));
    c.change_evaluated(
        "revoke",
        change,
        "reviewed",
        authorization(&policy, &stamp, &time),
    )
    .await
    .unwrap();
    assert!(
        c.snapshot()
            .await
            .unwrap()
            .revoked_subjects
            .contains("participant")
    );
}

#[tokio::test]
async fn intervention_does_not_allow_permit_publication() {
    let c = coordinator(Arc::new(MemoryStateStore::new())).await;
    let snapshot = c.snapshot().await.unwrap();
    let change = AuthorityChange::PublishPermit {
        permit: ExecutionPermit {
            id: "permit".into(),
            revision: 1,
            subject: actor("participant"),
            effects: vec![acteon_governance::context::AcceptedEffect {
                operation: "work".into(),
                resources: ceiling().resources,
            }],
            valid_from_ms: 0,
            limits: RootBudgetLimits {
                max_units: 10,
                max_concurrent: 1,
                deadline_ms: 900,
            },
        },
    };
    assert!(
        c.change_evaluated(
            "issue",
            change,
            "reviewed",
            authorization(&ceiling(), &snapshot.stamp(), &clock())
        )
        .await
        .is_err()
    );
    assert!(c.snapshot().await.unwrap().permits.is_empty());
    assert_eq!(c.snapshot().await.unwrap().generation, snapshot.generation);
}
