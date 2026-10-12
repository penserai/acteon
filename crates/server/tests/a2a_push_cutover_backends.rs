use std::sync::Arc;

use acteon_core::{DlqFailureKind, PushDeliveryDlqEntry, Task, TaskPushNotificationConfig};
use acteon_gateway::{TaskEngine, TaskScope};
use acteon_server::a2a_push_cutover::{
    A2aPushCutoverError, A2aPushCutoverPlan, a2a_push_cutover_complete,
    verify_a2a_push_cutover_fence,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

#[allow(clippy::too_many_lines)] // one linear cross-backend migration scenario
async fn cutover_contract(state: Arc<dyn StateStore>) {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let namespace = format!("cutover-{suffix}");
    let tenant = "backend-contract";
    let task = Task::new(format!("task-{suffix}"), &namespace, tenant);
    let scope = TaskScope::new(&namespace, tenant);
    let engine = TaskEngine::new(state.clone());
    state
        .set(
            &StateKey::new(namespace.as_str(), tenant, KeyKind::A2aTask, &task.id),
            &serde_json::to_string(&task).unwrap(),
            None,
        )
        .await
        .unwrap();

    let mut old_owned = callback(&task, "old-owned");
    let native_edit_source = callback(&task, "native-edit");
    let native_delete_source = callback(&task, "native-delete");
    for config in [&old_owned, &native_edit_source, &native_delete_source] {
        state
            .set(
                &config_key(config),
                &serde_json::to_string(config).unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    engine
        .import_legacy_push_configs(
            &scope,
            &task.id,
            vec![
                old_owned.clone(),
                native_edit_source.clone(),
                native_delete_source.clone(),
            ],
        )
        .await
        .unwrap();

    let mut native_edit = native_edit_source.clone();
    native_edit.url = "https://native.example/callback".into();
    native_edit.updated_at += chrono::Duration::seconds(1);
    engine
        .upsert_push_config(&scope, &task.id, native_edit.clone())
        .await
        .unwrap();
    assert!(
        engine
            .delete_push_config(&scope, &task.id, &native_delete_source.id)
            .await
            .unwrap()
    );

    old_owned.url = "https://old-writer.example/callback".into();
    old_owned.updated_at += chrono::Duration::seconds(2);
    state
        .set(
            &config_key(&old_owned),
            &serde_json::to_string(&old_owned).unwrap(),
            None,
        )
        .await
        .unwrap();

    let dead_letter = PushDeliveryDlqEntry::new(
        uuid::Uuid::now_v7().to_string(),
        &old_owned.id,
        &task.id,
        &task.namespace,
        &task.tenant,
        &old_owned.url,
        DlqFailureKind::Exhausted,
        "receiver unavailable",
        3,
        r#"{"status":"failed"}"#.into(),
    );
    let dead_letter_key = StateKey::new(
        namespace.as_str(),
        tenant,
        KeyKind::A2aPushDlq,
        dead_letter.storage_id(),
    );
    state
        .set(
            &dead_letter_key,
            &serde_json::to_string(&dead_letter).unwrap(),
            None,
        )
        .await
        .unwrap();

    let plan = A2aPushCutoverPlan::preview(
        state.clone(),
        "backend-contract",
        "qualify portable callback cutover",
    )
    .await
    .unwrap();
    assert_eq!(plan.report().task_count, 1);
    assert_eq!(plan.report().legacy_config_count, 3);
    assert_eq!(plan.report().legacy_dead_letter_count, 1);
    let report = serde_json::to_string(plan.report()).unwrap();
    assert!(!report.contains("callback-secret"));
    assert!(plan.apply(&plan.report().review_digest).await.unwrap());
    assert!(a2a_push_cutover_complete(state.as_ref()).await.unwrap());

    let configs = engine
        .list_push_configs(&scope, &task.id)
        .await
        .unwrap()
        .into_iter()
        .map(|config| (config.id.clone(), config))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(configs["old-owned"].url, old_owned.url);
    assert_eq!(configs["native-edit"].url, native_edit.url);
    assert!(!configs.contains_key("native-delete"));
    assert_eq!(
        engine
            .list_push_dead_letters(&scope, &task.id)
            .await
            .unwrap(),
        vec![dead_letter]
    );
    assert_eq!(
        state
            .scan_keys_by_kind(KeyKind::A2aTaskPushConfig)
            .await
            .unwrap(),
        Vec::<(String, String)>::new()
    );
    assert_eq!(
        state.scan_keys_by_kind(KeyKind::A2aPushDlq).await.unwrap(),
        Vec::<(String, String)>::new()
    );

    state
        .set(
            &config_key(&native_edit),
            &serde_json::to_string(&native_edit).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        verify_a2a_push_cutover_fence(state.as_ref()).await,
        Err(A2aPushCutoverError::PostCutoverLegacyRows)
    ));
}

fn callback(task: &Task, id: &str) -> TaskPushNotificationConfig {
    let mut config = TaskPushNotificationConfig::new(
        id,
        &task.id,
        &task.namespace,
        &task.tenant,
        format!("https://{id}.example/callback"),
    );
    config.token = Some("callback-secret".into());
    config
}

fn config_key(config: &TaskPushNotificationConfig) -> StateKey {
    StateKey::new(
        config.namespace.as_str(),
        config.tenant.as_str(),
        KeyKind::A2aTaskPushConfig,
        config.storage_id(),
    )
}

#[tokio::test]
async fn memory_a2a_push_cutover_contract() {
    cutover_contract(Arc::new(MemoryStateStore::new())).await;
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires REDIS_URL; isolated key prefix"]
async fn redis_a2a_push_cutover_contract() {
    use acteon_state_redis::{RedisConfig, RedisStateStore};
    let config = RedisConfig {
        url: std::env::var("REDIS_URL").unwrap(),
        prefix: format!("a2a-cutover-{}", uuid::Uuid::new_v4()),
        ..RedisConfig::default()
    };
    cutover_contract(Arc::new(RedisStateStore::new(&config).unwrap())).await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires DATABASE_URL; isolated table prefix"]
async fn postgres_a2a_push_cutover_contract() {
    use acteon_state_postgres::{PostgresConfig, PostgresStateStore};
    let config = PostgresConfig {
        url: std::env::var("DATABASE_URL").unwrap(),
        table_prefix: format!("a2a_cutover_{}_", uuid::Uuid::new_v4().simple()),
        ..PostgresConfig::default()
    };
    let store = Arc::new(PostgresStateStore::new(config.clone()).await.unwrap());
    cutover_contract(store).await;
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

#[cfg(feature = "dynamodb")]
#[tokio::test]
#[ignore = "requires DYNAMODB_ENDPOINT; isolated DynamoDB Local table"]
async fn dynamodb_a2a_push_cutover_contract() {
    use acteon_state_dynamodb::{DynamoConfig, DynamoStateStore, build_client, create_table};
    let config = DynamoConfig {
        endpoint_url: Some(std::env::var("DYNAMODB_ENDPOINT").unwrap()),
        table_name: format!("a2a_cutover_{}", uuid::Uuid::new_v4().simple()),
        key_prefix: format!("a2a-cutover-{}", uuid::Uuid::new_v4()),
        ..DynamoConfig::default()
    };
    let client = build_client(&config).await;
    create_table(&client, &config.table_name).await.unwrap();
    let store = Arc::new(DynamoStateStore::new(&config).await.unwrap());
    cutover_contract(store).await;
    client
        .delete_table()
        .table_name(&config.table_name)
        .send()
        .await
        .unwrap();
}
