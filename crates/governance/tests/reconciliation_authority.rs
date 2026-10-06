use std::{sync::Arc, time::Duration};

use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::{
    AttemptEvidenceReference, AttemptReconciliationReference, AttemptRequest, AttemptStatus,
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, COORDINATOR_KIND, CoordinationError,
    CoordinatorLimits, RootBudgetLimits, RootReservation, ScopePurpose, StartRegistration,
    reconciliation::{ReconciliationAuthorization, ReconciliationCeiling},
};
use acteon_state::{
    KeyKind, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::MemoryStateStore;
use acteon_time::ManualClock;

fn resource(id: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::Endpoint, "city", "tenant", id).unwrap()
}
fn ceiling() -> ReconciliationCeiling {
    ReconciliationCeiling {
        actor: PrincipalIdentity::new("operator", PrincipalKind::Human).unwrap(),
        subjects: vec!["agent".into()],
        resources: vec![resource("building"), resource("utility")],
        valid_from_ms: 0,
        deadline_ms: 1000,
    }
}
fn authorization<'a>(
    ceiling: &'a ReconciliationCeiling,
    stamp: &'a AuthorityStamp,
    clock: &'a ManualClock,
) -> ReconciliationAuthorization<'a> {
    ReconciliationAuthorization {
        ceiling,
        evaluated_authority: stamp,
        clock,
        guard: None,
    }
}
fn evidence() -> AttemptReconciliationReference {
    AttemptReconciliationReference {
        prior_status: AttemptStatus::Uncertain,
        original_evidence: Some(AttemptEvidenceReference {
            id: "original".into(),
            digest: "b".repeat(64),
        }),
        resolution: AttemptEvidenceReference {
            id: "attempt".into(),
            digest: "a".repeat(64),
        },
    }
}
async fn fixture() -> (AuthorityCoordinator, Arc<FaultStore>, String, ManualClock) {
    let faults = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
    let coordinator = AuthorityCoordinator::initialize(
        faults.clone(),
        "city",
        "tenant",
        CoordinatorLimits::default(),
    )
    .await
    .unwrap();
    coordinator
        .reserve_scope(ScopePurpose::Execution)
        .await
        .unwrap();
    coordinator
        .create_root_budget(
            "root",
            "agent",
            RootBudgetLimits {
                max_units: 3,
                max_concurrent: 1,
                deadline_ms: 1000,
            },
            &coordinator.snapshot().await.unwrap().stamp(),
            100,
        )
        .await
        .unwrap();
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let StartRegistration::New(start) = coordinator
        .register_attempt(AttemptRequest {
            id: "attempt",
            subject: "agent",
            resources: &[resource("building"), resource("utility")],
            request_digest: "input",
            expected_authority: &stamp,
            reservation: Some(RootReservation {
                root_id: "root".into(),
                units: 2,
            }),
            now_ms: 100,
        })
        .await
        .unwrap()
    else {
        panic!("fresh attempt expected")
    };
    coordinator
        .settle_with_evidence(
            "attempt",
            &start.token,
            AttemptStatus::Uncertain,
            evidence().original_evidence.unwrap(),
        )
        .await
        .unwrap();
    let clock = ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap());
    (coordinator, faults, start.token, clock)
}

#[tokio::test]
async fn full_footprint_and_subject_are_required_before_settlement_and_replay() {
    let (coordinator, _, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let mut policy = ceiling();
    for wrong_subject in [false, true] {
        if wrong_subject {
            policy = ceiling();
            policy.subjects = vec!["other".into()];
        } else {
            policy.resources.pop();
        }
        assert!(matches!(
            coordinator
                .reconcile_attempt_evaluated(
                    "attempt",
                    &token,
                    evidence(),
                    authorization(&policy, &stamp, &clock),
                )
                .await,
            Err(CoordinationError::Restricted)
        ));
        assert_eq!(
            coordinator.snapshot().await.unwrap().starts["attempt"].status,
            AttemptStatus::Uncertain
        );
    }
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].spent_units, 2);
    assert_eq!(state.roots["root"].active_attempts, 1);
    policy = ceiling();
    coordinator
        .reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&policy, &stamp, &clock),
        )
        .await
        .unwrap();
    coordinator
        .reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&policy, &stamp, &clock),
        )
        .await
        .unwrap();
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.roots["root"].spent_units, 2);
    assert_eq!(state.roots["root"].active_attempts, 0);
    assert_eq!(
        state.starts["attempt"].evidence,
        evidence().original_evidence
    );
    assert_eq!(state.starts["attempt"].reconciliation, Some(evidence()));
    clock.advance_to(Duration::from_millis(900)).unwrap();
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&policy, &stamp, &clock),
            )
            .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn closure_wins_cas_race_and_fresh_authority_can_accept_past_finality() {
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let resume = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let peer = coordinator.clone();
    let token_for_task = token.clone();
    let task = tokio::spawn(async move {
        peer.reconcile_attempt_evaluated(
            "attempt",
            &token_for_task,
            evidence(),
            authorization(&ceiling(), &stamp, &clock),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    coordinator
        .change(
            "closure",
            AuthorityChange::CloseResource {
                resource: resource("building"),
            },
            "police",
            "maintenance",
        )
        .await
        .unwrap();
    resume.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::StaleAuthority)
    ));
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.starts["attempt"].status, AttemptStatus::Uncertain);
    assert!(state.starts["attempt"].reconciliation.is_none());
    let clock = ManualClock::new(chrono::DateTime::from_timestamp_millis(100).unwrap());
    coordinator
        .reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&ceiling(), &state.stamp(), &clock),
        )
        .await
        .unwrap();
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.starts.len(), 1);
    assert!(state.closed_resources.contains(&resource("building")));
    assert_eq!(state.starts["attempt"].status, AttemptStatus::Settled);
}

#[tokio::test]
async fn lost_ack_replay_still_checks_revoked_operator() {
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&ceiling(), &stamp, &clock)
            )
            .await
            .is_err()
    );
    assert_eq!(
        coordinator.snapshot().await.unwrap().starts["attempt"].status,
        AttemptStatus::Settled
    );
    coordinator
        .change(
            "offboard",
            AuthorityChange::RevokeSubject {
                subject: "operator".into(),
            },
            "admin",
            "offboard",
        )
        .await
        .unwrap();
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&ceiling(), &stamp, &clock)
            )
            .await,
        Err(CoordinationError::Restricted)
    ));
}

#[tokio::test]
async fn expiry_is_rechecked_after_cas_conflict_without_releasing_capacity() {
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let resume = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let peer = coordinator.clone();
    let time = clock.clone();
    let evaluated = stamp.clone();
    let task = tokio::spawn(async move {
        peer.reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&ceiling(), &evaluated, &time),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // A budget mutation changes the CAS version without changing authority.
    coordinator
        .create_root_budget(
            "other",
            "other",
            RootBudgetLimits {
                max_units: 1,
                max_concurrent: 1,
                deadline_ms: 1000,
            },
            &coordinator.snapshot().await.unwrap().stamp(),
            100,
        )
        .await
        .unwrap();
    clock.advance_to(Duration::from_millis(900)).unwrap();
    resume.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::Restricted)
    ));
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.stamp(), stamp);
    assert_eq!(state.starts["attempt"].status, AttemptStatus::Uncertain);
    assert!(state.starts["attempt"].reconciliation.is_none());
    assert_eq!(state.roots["root"].spent_units, 2);
    assert_eq!(state.roots["root"].active_attempts, 1);
}

#[tokio::test]
async fn invalid_bounds_and_wrong_token_leave_original_attempt_intact() {
    let (coordinator, _, token, clock) = fixture().await;
    let before = coordinator.snapshot().await.unwrap();
    let stamp = before.stamp();
    let mut policy = ceiling();
    policy.subjects.push("agent".into());
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&policy, &stamp, &clock)
            )
            .await,
        Err(CoordinationError::Invalid(_))
    ));
    policy = ceiling();
    policy.valid_from_ms = 101;
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&policy, &stamp, &clock)
            )
            .await,
        Err(CoordinationError::Restricted)
    ));
    policy = ceiling();
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                "wrong-token",
                evidence(),
                authorization(&policy, &stamp, &clock)
            )
            .await,
        Err(CoordinationError::Conflict)
    ));
    assert_eq!(
        serde_json::to_value(coordinator.snapshot().await.unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[tokio::test]
async fn acceptance_audit_commits_atomically_and_replay_preserves_original_attribution() {
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let mut policy = ceiling();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&policy, &stamp, &clock)
            )
            .await
            .is_err()
    );
    let refused = coordinator.snapshot().await.unwrap();
    assert!(refused.starts["attempt"].reconciliation.is_none());
    assert!(
        refused.starts["attempt"]
            .reconciliation_acceptance
            .is_none()
    );
    assert_eq!(refused.roots["root"].active_attempts, 1);
    clock.advance_to(Duration::from_millis(100)).unwrap();
    faults
        .fail_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        coordinator
            .reconcile_attempt_evaluated(
                "attempt",
                &token,
                evidence(),
                authorization(&policy, &stamp, &clock)
            )
            .await
            .is_err()
    );
    let settled = coordinator.snapshot().await.unwrap();
    let recorded = settled.starts["attempt"]
        .reconciliation_acceptance
        .clone()
        .unwrap();
    assert_eq!(
        recorded.operator,
        PrincipalIdentity::new("operator", PrincipalKind::Human).unwrap()
    );
    assert_eq!(recorded.authority, stamp);
    assert_eq!(recorded.accepted_at_ms, 200);
    clock.advance_to(Duration::from_millis(300)).unwrap();
    policy.actor = PrincipalIdentity::new("replacement", PrincipalKind::Human).unwrap();
    coordinator
        .reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&policy, &stamp, &clock),
        )
        .await
        .unwrap();
    let replayed = coordinator.snapshot().await.unwrap();
    assert_eq!(
        replayed.starts["attempt"].reconciliation_acceptance,
        Some(recorded)
    );
    assert_eq!(
        (
            replayed.roots["root"].spent_units,
            replayed.roots["root"].active_attempts
        ),
        (2, 0)
    );
}

#[tokio::test]
async fn corrupt_acceptance_audit_fails_closed_on_recovery() {
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    coordinator
        .reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            authorization(&ceiling(), &stamp, &clock),
        )
        .await
        .unwrap();
    let key = acteon_state::StateKey::new(
        "city",
        "tenant",
        KeyKind::Custom(COORDINATOR_KIND.into()),
        "authority",
    );
    let raw = faults.get(&key).await.unwrap().unwrap();
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for corruption in [
        "future_generation",
        "foreign_incarnation",
        "negative_time",
        "orphan",
    ] {
        let mut data = original.clone();
        let start = &mut data["starts"]["attempt"];
        match corruption {
            "future_generation" => {
                start["reconciliation_acceptance"]["authority"]["generation"] =
                    serde_json::json!(u64::MAX);
            }
            "foreign_incarnation" => {
                start["reconciliation_acceptance"]["authority"]["incarnation"] =
                    serde_json::json!("other-scope");
            }
            "negative_time" => {
                start["reconciliation_acceptance"]["accepted_at_ms"] = serde_json::json!(-1);
            }
            _ => start["reconciliation"] = serde_json::Value::Null,
        }
        faults.set(&key, &data.to_string(), None).await.unwrap();
        assert!(
            coordinator.snapshot().await.is_err(),
            "{corruption} was accepted"
        );
    }
    faults.set(&key, &raw, None).await.unwrap();
    assert!(coordinator.snapshot().await.is_ok());
}

struct FreshnessGuard {
    allowed: std::sync::atomic::AtomicBool,
    checks: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl acteon_governance::reconciliation::ReconciliationAuthorityGuard for FreshnessGuard {
    async fn check_current(&self) -> Result<(), CoordinationError> {
        self.checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.allowed.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(CoordinationError::Restricted)
        }
    }
}
#[tokio::test]
async fn host_freshness_guard_controls_preflight_acceptance_and_replay() {
    use std::sync::atomic::Ordering;
    let (coordinator, _, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let policy = ceiling();
    let guard = FreshnessGuard {
        allowed: false.into(),
        checks: 0.into(),
    };
    let evaluated = || ReconciliationAuthorization {
        ceiling: &policy,
        evaluated_authority: &stamp,
        clock: &clock,
        guard: Some(&guard),
    };
    assert!(matches!(
        coordinator
            .check_reconciliation_authorization("attempt", &evaluated())
            .await,
        Err(CoordinationError::Restricted)
    ));
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated("attempt", &token, evidence(), evaluated())
            .await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        coordinator.snapshot().await.unwrap().starts["attempt"].status,
        AttemptStatus::Uncertain
    );
    guard.allowed.store(true, Ordering::SeqCst);
    coordinator
        .reconcile_attempt_evaluated("attempt", &token, evidence(), evaluated())
        .await
        .unwrap();
    let accepted = coordinator.snapshot().await.unwrap();
    guard.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        coordinator
            .reconcile_attempt_evaluated("attempt", &token, evidence(), evaluated())
            .await,
        Err(CoordinationError::Restricted)
    ));
    assert_eq!(
        coordinator.snapshot().await.unwrap().starts["attempt"].reconciliation_acceptance,
        accepted.starts["attempt"].reconciliation_acceptance
    );
    assert_eq!(guard.checks.load(Ordering::SeqCst), 4);
}
#[tokio::test]
async fn host_freshness_guard_is_rechecked_after_a_version_only_cas_conflict() {
    use std::sync::atomic::Ordering;
    let (coordinator, faults, token, clock) = fixture().await;
    let stamp = coordinator.snapshot().await.unwrap().stamp();
    let guard = Arc::new(FreshnessGuard {
        allowed: true.into(),
        checks: 0.into(),
    });
    let resume = faults
        .pause_next(
            KeyKind::Custom(COORDINATOR_KIND.into()),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let peer = coordinator.clone();
    let observed = stamp.clone();
    let current = guard.clone();
    let task = tokio::spawn(async move {
        peer.reconcile_attempt_evaluated(
            "attempt",
            &token,
            evidence(),
            ReconciliationAuthorization {
                ceiling: &ceiling(),
                evaluated_authority: &observed,
                clock: &clock,
                guard: Some(current.as_ref()),
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while faults.consumed() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    coordinator
        .create_root_budget(
            "other",
            "other",
            RootBudgetLimits {
                max_units: 1,
                max_concurrent: 1,
                deadline_ms: 1000,
            },
            &stamp,
            100,
        )
        .await
        .unwrap();
    guard.allowed.store(false, Ordering::SeqCst);
    resume.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CoordinationError::Restricted)
    ));
    let state = coordinator.snapshot().await.unwrap();
    assert_eq!(state.stamp(), stamp);
    assert_eq!(state.starts["attempt"].status, AttemptStatus::Uncertain);
    assert_eq!(state.roots["root"].active_attempts, 1);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 2);
}
