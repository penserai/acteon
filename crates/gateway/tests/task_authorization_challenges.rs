use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use futures::FutureExt;
use sha2::Digest;

use acteon_core::{
    BusApproval, BusApprovalStatus, PrincipalIdentity, PrincipalKind, Task,
    TaskAuthorizationRequirement, TaskAuthorizationResolution, TaskState,
};
use acteon_gateway::{
    TaskAuthorizationVerification, TaskAuthorizationVerificationError, TaskAuthorizationVerifier,
    TaskEngine, TaskEngineError, TaskScope, VerifiedTaskAuthorization,
};
use acteon_state::{KeyKind, StateKey, StateStore};

struct Verifier {
    calls: AtomicUsize,
}

#[async_trait]
impl TaskAuthorizationVerifier for Verifier {
    fn verifier_id(&self) -> &'static str {
        "workload-identity"
    }

    fn revision(&self) -> u64 {
        7
    }

    async fn verify(
        &self,
        request: &TaskAuthorizationVerification,
    ) -> Result<VerifiedTaskAuthorization, TaskAuthorizationVerificationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.scope, TaskScope::new("city", "auth-contract"));
        assert_ne!(request.task_id, "");
        assert_ne!(request.challenge_id, "");
        assert_eq!(
            request.requirement.authorization_request_id,
            "authorization-session-42"
        );
        assert_eq!(request.requirement.credential_authority, "vault-prod");
        let now = Utc::now();
        Ok(VerifiedTaskAuthorization {
            decision_id: "decision-42".into(),
            subject: PrincipalIdentity::new("alice", PrincipalKind::Human).unwrap(),
            verified_at: now,
            valid_until: now + Duration::minutes(5),
        })
    }
}

fn requirement() -> TaskAuthorizationRequirement {
    TaskAuthorizationRequirement {
        verifier_id: "workload-identity".into(),
        verifier_revision: 7,
        authorization_request_id: "authorization-session-42".into(),
        recipient: PrincipalIdentity::new("diagnostic-agent", PrincipalKind::Agent).unwrap(),
        credential_authority: "vault-prod".into(),
        audience: "incident-api".into(),
        required_scopes: vec!["incidents.read".into(), "traces.read".into()],
    }
}

async fn independent_clients_authorize_once(
    writer: Arc<dyn StateStore>,
    resolver: Arc<dyn StateStore>,
) {
    let scope = TaskScope::new("city", "auth-contract");
    let writer = TaskEngine::new(writer);
    let resolver = TaskEngine::new(resolver);
    let task_id = format!("task-{}", uuid::Uuid::new_v4());

    writer
        .create_task(Task::new(&task_id, "city", "auth-contract"))
        .await
        .unwrap();
    writer
        .transition_task(&scope, &task_id, TaskState::Working, None)
        .await
        .unwrap();
    let (_, challenge) = writer
        .pause_for_authorization(
            &scope,
            &task_id,
            requirement(),
            Some("Connect your incident-management identity".into()),
            None,
        )
        .await
        .unwrap();

    let verifier = Verifier {
        calls: AtomicUsize::new(0),
    };
    let (task, approval) = resolver
        .resolve_authorization(&scope, &task_id, &challenge.approval_id, &verifier)
        .await
        .unwrap();
    assert_eq!(task.status.state, TaskState::Working);
    assert!(task.pending_approval_id.is_none());
    assert!(
        task.history.is_empty(),
        "authorization data is not task input"
    );
    assert_eq!(approval.status, BusApprovalStatus::Approved);
    let resolution = approval.authorization_resolution.as_ref().unwrap();
    assert_eq!(resolution.decision_id, "decision-42");
    assert_eq!(resolution.subject.id(), "alice");
    assert_eq!(resolution.authorization_request_digest.len(), 64);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);

    // Lost-response retry finalizes/observes the already committed exact
    // decision and never calls the external verifier a second time.
    let (retried, retried_approval) = writer
        .resolve_authorization(&scope, &task_id, &challenge.approval_id, &verifier)
        .await
        .unwrap();
    assert_eq!(retried.status.state, TaskState::Working);
    assert_eq!(retried_approval.status, BusApprovalStatus::Approved);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn memory_clients_pass_the_authorization_challenge_contract() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    independent_clients_authorize_once(Arc::clone(&state), state).await;
}

#[tokio::test]
async fn unbound_auth_pause_cannot_be_promoted_to_authority() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let engine = TaskEngine::new(Arc::clone(&state));
    let scope = TaskScope::new("city", "auth-contract");
    engine
        .create_task(Task::new("legacy", "city", "auth-contract"))
        .await
        .unwrap();
    engine
        .transition_task(&scope, "legacy", TaskState::Working, None)
        .await
        .unwrap();
    assert!(matches!(
        engine
            .pause_for_human(
                &scope,
                "legacy",
                acteon_core::PauseKind::UserAuth,
                None,
                None,
            )
            .await
            .unwrap_err(),
        TaskEngineError::AuthorizationRequirementRequired
    ));
    let (_, challenge) = engine
        .pause_for_authorization(&scope, "legacy", requirement(), None, None)
        .await
        .unwrap();
    let key = StateKey::new(
        "city",
        "auth-contract",
        KeyKind::BusApproval,
        &challenge.approval_id,
    );
    let mut legacy: BusApproval =
        serde_json::from_str(&state.get(&key).await.unwrap().unwrap()).unwrap();
    legacy.authorization_requirement = None;
    legacy.validate().unwrap();
    state
        .set(&key, &serde_json::to_string(&legacy).unwrap(), None)
        .await
        .unwrap();
    let verifier = Verifier {
        calls: AtomicUsize::new(0),
    };
    let error = engine
        .resolve_authorization(&scope, "legacy", &challenge.approval_id, &verifier)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        TaskEngineError::AuthorizationChallengeUnbound(_)
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine
            .get_task(&scope, "legacy")
            .await
            .unwrap()
            .unwrap()
            .status
            .state,
        TaskState::AuthRequired
    );
}

struct DenyingVerifier;

#[async_trait]
impl TaskAuthorizationVerifier for DenyingVerifier {
    fn verifier_id(&self) -> &'static str {
        "workload-identity"
    }

    fn revision(&self) -> u64 {
        7
    }

    async fn verify(
        &self,
        _request: &TaskAuthorizationVerification,
    ) -> Result<VerifiedTaskAuthorization, TaskAuthorizationVerificationError> {
        Err(TaskAuthorizationVerificationError::Denied)
    }
}

#[tokio::test]
async fn verifier_denial_leaves_the_challenge_pending_and_task_paused() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let engine = TaskEngine::new(Arc::clone(&state));
    let scope = TaskScope::new("city", "auth-contract");
    engine
        .create_task(Task::new("denied", "city", "auth-contract"))
        .await
        .unwrap();
    engine
        .transition_task(&scope, "denied", TaskState::Working, None)
        .await
        .unwrap();
    let (_, challenge) = engine
        .pause_for_authorization(&scope, "denied", requirement(), None, None)
        .await
        .unwrap();

    assert!(matches!(
        engine
            .resolve_authorization(&scope, "denied", &challenge.approval_id, &DenyingVerifier)
            .await
            .unwrap_err(),
        TaskEngineError::AuthorizationVerification(TaskAuthorizationVerificationError::Denied)
    ));
    assert_eq!(
        engine
            .get_task(&scope, "denied")
            .await
            .unwrap()
            .unwrap()
            .status
            .state,
        TaskState::AuthRequired
    );
    let key = StateKey::new(
        "city",
        "auth-contract",
        KeyKind::BusApproval,
        &challenge.approval_id,
    );
    let approval: BusApproval =
        serde_json::from_str(&state.get(&key).await.unwrap().unwrap()).unwrap();
    assert_eq!(approval.status, BusApprovalStatus::Pending);
    assert!(approval.authorization_resolution.is_none());
}

#[tokio::test]
async fn retry_refreshes_current_evidence_after_the_pending_deadline() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let engine = TaskEngine::new(Arc::clone(&state));
    let scope = TaskScope::new("city", "auth-contract");
    engine
        .create_task(Task::new("recover", "city", "auth-contract"))
        .await
        .unwrap();
    engine
        .transition_task(&scope, "recover", TaskState::Working, None)
        .await
        .unwrap();
    let (_, challenge) = engine
        .pause_for_authorization(&scope, "recover", requirement(), None, None)
        .await
        .unwrap();

    // Model a crash after the approval claim but before the task CAS. The old
    // evidence is now stale, while its stable decision identity is unchanged.
    let key = StateKey::new(
        "city",
        "auth-contract",
        KeyKind::BusApproval,
        &challenge.approval_id,
    );
    let mut claimed: BusApproval =
        serde_json::from_str(&state.get(&key).await.unwrap().unwrap()).unwrap();
    let now = Utc::now();
    claimed.status = BusApprovalStatus::Approving;
    claimed.authorization_resolution = Some(TaskAuthorizationResolution {
        verifier_id: "workload-identity".into(),
        verifier_revision: 7,
        authorization_request_digest: hex::encode(sha2::Sha256::digest(
            b"authorization-session-42",
        )),
        decision_id: "decision-42".into(),
        subject: PrincipalIdentity::new("alice", PrincipalKind::Human).unwrap(),
        verified_at: now - Duration::minutes(10),
        valid_until: now - Duration::minutes(5),
    });
    claimed.decided_by = Some("alice".into());
    claimed.decided_at = Some(now - Duration::minutes(10));
    // The decision was claimed before this deadline. Once Approving, the row
    // must remain recoverable instead of racing a task commit into Expired.
    claimed.created_at = now - Duration::minutes(20);
    claimed.expires_at = now - Duration::minutes(5);
    claimed.validate().unwrap();
    state
        .set(&key, &serde_json::to_string(&claimed).unwrap(), None)
        .await
        .unwrap();

    let verifier = Verifier {
        calls: AtomicUsize::new(0),
    };
    let (task, approval) = engine
        .resolve_authorization(&scope, "recover", &challenge.approval_id, &verifier)
        .await
        .unwrap();
    assert_eq!(task.status.state, TaskState::Working);
    assert_eq!(approval.status, BusApprovalStatus::Approved);
    assert!(approval.authorization_resolution.unwrap().valid_until > now);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
}

struct ConcurrentVerifier {
    barrier: tokio::sync::Barrier,
    calls: AtomicUsize,
}

#[async_trait]
impl TaskAuthorizationVerifier for ConcurrentVerifier {
    fn verifier_id(&self) -> &'static str {
        "workload-identity"
    }

    fn revision(&self) -> u64 {
        7
    }

    async fn verify(
        &self,
        _request: &TaskAuthorizationVerification,
    ) -> Result<VerifiedTaskAuthorization, TaskAuthorizationVerificationError> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst);
        self.barrier.wait().await;
        let now = Utc::now();
        Ok(VerifiedTaskAuthorization {
            decision_id: "decision-42".into(),
            subject: PrincipalIdentity::new("alice", PrincipalKind::Human).unwrap(),
            verified_at: now - Duration::seconds(i64::try_from(ordinal).unwrap()),
            valid_until: now
                + Duration::minutes(5)
                + Duration::seconds(i64::try_from(ordinal).unwrap()),
        })
    }
}

#[tokio::test]
async fn concurrent_current_evidence_refreshes_converge_on_one_decision() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let engine = TaskEngine::new(state);
    let scope = TaskScope::new("city", "auth-contract");
    engine
        .create_task(Task::new("concurrent", "city", "auth-contract"))
        .await
        .unwrap();
    engine
        .transition_task(&scope, "concurrent", TaskState::Working, None)
        .await
        .unwrap();
    let (_, challenge) = engine
        .pause_for_authorization(&scope, "concurrent", requirement(), None, None)
        .await
        .unwrap();
    let verifier = Arc::new(ConcurrentVerifier {
        barrier: tokio::sync::Barrier::new(2),
        calls: AtomicUsize::new(0),
    });
    let left_engine = engine.clone();
    let left_scope = scope.clone();
    let left_challenge = challenge.approval_id.clone();
    let left_verifier = Arc::clone(&verifier);
    let left = tokio::spawn(async move {
        left_engine
            .resolve_authorization(
                &left_scope,
                "concurrent",
                &left_challenge,
                left_verifier.as_ref(),
            )
            .await
    });
    let right = engine.resolve_authorization(
        &scope,
        "concurrent",
        &challenge.approval_id,
        verifier.as_ref(),
    );
    let (left, right) = tokio::join!(left, right);
    let (_, left_approval) = left.unwrap().unwrap();
    let (_, right_approval) = right.unwrap();
    assert_eq!(left_approval.status, BusApprovalStatus::Approved);
    assert_eq!(right_approval.status, BusApprovalStatus::Approved);
    assert_eq!(
        left_approval
            .authorization_resolution
            .as_ref()
            .unwrap()
            .decision_id,
        right_approval
            .authorization_resolution
            .as_ref()
            .unwrap()
            .decision_id
    );
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn expired_authorization_challenge_fails_the_still_paused_task() {
    let state: Arc<dyn StateStore> = Arc::new(acteon_state_memory::MemoryStateStore::new());
    let clock = Arc::new(acteon_time::ManualClock::new(Utc::now()));
    let engine = TaskEngine::new(Arc::clone(&state)).with_clock(clock.clone());
    let scope = TaskScope::new("city", "auth-contract");
    engine
        .create_task(Task::new_at(
            "expired",
            "city",
            "auth-contract",
            acteon_time::Clock::now(clock.as_ref()),
        ))
        .await
        .unwrap();
    engine
        .transition_task(&scope, "expired", TaskState::Working, None)
        .await
        .unwrap();
    let (_, challenge) = engine
        .pause_for_authorization(
            &scope,
            "expired",
            requirement(),
            None,
            Some(StdDuration::from_secs(1)),
        )
        .await
        .unwrap();
    clock.advance_to(StdDuration::from_secs(2)).unwrap();
    let verifier = Verifier {
        calls: AtomicUsize::new(0),
    };
    assert!(matches!(
        engine
            .resolve_authorization(&scope, "expired", &challenge.approval_id, &verifier)
            .await
            .unwrap_err(),
        TaskEngineError::ChallengeExpired(_)
    ));
    assert_eq!(
        engine
            .get_task(&scope, "expired")
            .await
            .unwrap()
            .unwrap()
            .status
            .state,
        TaskState::Failed
    );
    let key = StateKey::new(
        "city",
        "auth-contract",
        KeyKind::BusApproval,
        &challenge.approval_id,
    );
    let approval: BusApproval =
        serde_json::from_str(&state.get(&key).await.unwrap().unwrap()).unwrap();
    assert_eq!(approval.status, BusApprovalStatus::Expired);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; independent configured Redis clients"]
async fn independent_redis_clients_pass_the_authorization_challenge_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};

    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("task-auth-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let writer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let resolver: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    independent_clients_authorize_once(writer, resolver).await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL; independent configured PostgreSQL clients"]
async fn independent_postgres_clients_pass_the_authorization_challenge_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};

    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("task_auth_{}__", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let writer: Arc<dyn StateStore> =
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let resolver: Arc<dyn StateStore> =
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let contract =
        std::panic::AssertUnwindSafe(independent_clients_authorize_once(writer, resolver));
    let result = contract.catch_unwind().await;

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
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
