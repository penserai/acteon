use std::sync::Arc;

use acteon_core::{BusApprovalStatus, PauseKind, Task, TaskMessage, TaskRole, TaskState};
use acteon_gateway::{TaskEngine, TaskScope};
use acteon_state::StateStore;

async fn independent_clients_resolve_once(
    writer: Arc<dyn StateStore>,
    resolver: Arc<dyn StateStore>,
) {
    let scope = TaskScope::new("city", "input-contract");
    let writer = TaskEngine::new(writer);
    let resolver = TaskEngine::new(resolver);
    let task_id = format!("task-{}", uuid::Uuid::new_v4());

    let submitted = writer
        .create_task(Task::new(&task_id, "city", "input-contract"))
        .await
        .unwrap();
    let working = writer
        .transition_task(&scope, &task_id, TaskState::Working, None)
        .await
        .unwrap();
    assert_eq!(submitted.context_id, working.context_id);
    let (_, challenge) = writer
        .pause_for_human(
            &scope,
            &task_id,
            PauseKind::UserInput,
            Some("Choose a deployment window".into()),
            None,
        )
        .await
        .unwrap();

    let mut response = TaskMessage::text("deployment-window", TaskRole::User, "02:00 UTC");
    response.task_id = Some(task_id.clone());
    response.context_id = working.context_id.clone();
    let (resolved, approval) = resolver
        .resolve_input(
            &scope,
            &task_id,
            &challenge.approval_id,
            response.clone(),
            "operator@example.com",
        )
        .await
        .unwrap();
    assert_eq!(resolved.status.state, TaskState::Working);
    assert_eq!(approval.status, BusApprovalStatus::Approved);

    // A client that only saw the original pause can retry after a lost
    // response. The persisted digest repairs/finalizes without a second
    // history append.
    let (retried, retried_approval) = writer
        .resolve_input(
            &scope,
            &task_id,
            &challenge.approval_id,
            response,
            "operator@example.com",
        )
        .await
        .unwrap();
    assert_eq!(retried_approval.status, BusApprovalStatus::Approved);
    assert_eq!(
        retried
            .history
            .iter()
            .filter(|message| message.message_id == "deployment-window")
            .count(),
        1
    );
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL; independent configured Redis clients"]
async fn independent_redis_clients_pass_the_input_challenge_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};

    let config = RedisConfig {
        url: std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap(),
        prefix: format!("task-input-{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };
    let writer: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    let resolver: Arc<dyn StateStore> = Arc::new(RedisStateStore::new(&config).unwrap());
    independent_clients_resolve_once(writer, resolver).await;
}

#[tokio::test]
#[ignore = "requires DATABASE_URL; independent configured PostgreSQL clients"]
async fn independent_postgres_clients_pass_the_input_challenge_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};

    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("task_input_{}_", uuid::Uuid::new_v4().simple()),
        ..Default::default()
    };
    let writer: Arc<dyn StateStore> =
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let resolver: Arc<dyn StateStore> =
        Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    let contract = std::panic::AssertUnwindSafe(independent_clients_resolve_once(writer, resolver));
    use futures::FutureExt;
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
