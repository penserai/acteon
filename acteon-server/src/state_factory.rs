use std::sync::Arc;

use acteon_state::{DistributedLock, StateStore};
#[cfg(feature = "clickhouse")]
use acteon_state_clickhouse::{ClickHouseConfig, ClickHouseDistributedLock, ClickHouseStateStore};
#[cfg(feature = "dynamodb")]
use acteon_state_dynamodb::{build_client, create_table, DynamoConfig, DynamoDistributedLock, DynamoStateStore};
#[cfg(feature = "etcd")]
use acteon_state_etcd::{EtcdConfig, EtcdDistributedLock, EtcdStateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
#[cfg(feature = "postgres")]
use acteon_state_postgres::{PostgresConfig, PostgresDistributedLock, PostgresStateStore};
#[cfg(feature = "redis")]
use acteon_state_redis::{RedisConfig, RedisDistributedLock, RedisStateStore};

use crate::config::StateConfig;
use crate::error::ServerError;

/// A state store and distributed lock pair.
pub type StatePair = (Arc<dyn StateStore>, Arc<dyn DistributedLock>);

/// Construct a `StateStore` and `DistributedLock` pair from configuration.
pub async fn create_state(config: &StateConfig) -> Result<StatePair, ServerError> {
    match config.backend.as_str() {
        "memory" => Ok(create_memory()),
        #[cfg(feature = "redis")]
        "redis" => create_redis(config),
        #[cfg(feature = "postgres")]
        "postgres" => create_postgres(config).await,
        #[cfg(feature = "dynamodb")]
        "dynamodb" => create_dynamodb(config).await,
        #[cfg(feature = "etcd")]
        "etcd" => create_etcd(config).await,
        #[cfg(feature = "clickhouse")]
        "clickhouse" => create_clickhouse(config).await,
        other => Err(ServerError::Config(format!(
            "unsupported state backend: {other} (is the feature enabled?)"
        ))),
    }
}

fn create_memory() -> StatePair {
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());
    (store, lock)
}

#[cfg(feature = "redis")]
fn create_redis(config: &StateConfig) -> Result<StatePair, ServerError> {
    let url = config.url.as_deref().unwrap_or("redis://127.0.0.1:6379");
    let redis_config = RedisConfig {
        url: url.to_owned(),
        prefix: config.prefix.clone().unwrap_or_else(|| "acteon".to_owned()),
        ..RedisConfig::default()
    };
    let store = Arc::new(
        RedisStateStore::new(&redis_config)
            .map_err(|e| ServerError::Config(format!("redis store: {e}")))?,
    );
    let lock = Arc::new(
        RedisDistributedLock::new(&redis_config)
            .map_err(|e| ServerError::Config(format!("redis lock: {e}")))?,
    );
    Ok((store, lock))
}

#[cfg(feature = "postgres")]
async fn create_postgres(config: &StateConfig) -> Result<StatePair, ServerError> {
    let url = config
        .url
        .as_deref()
        .ok_or_else(|| ServerError::Config("postgres backend requires 'url' in [state]".into()))?;
    let pg_config = PostgresConfig {
        url: url.to_owned(),
        table_prefix: config
            .prefix
            .clone()
            .unwrap_or_else(|| "acteon_".to_owned()),
        ..PostgresConfig::default()
    };
    let store = Arc::new(
        PostgresStateStore::new(pg_config.clone())
            .await
            .map_err(|e| ServerError::Config(format!("postgres store: {e}")))?,
    );
    let lock = Arc::new(
        PostgresDistributedLock::new(pg_config)
            .await
            .map_err(|e| ServerError::Config(format!("postgres lock: {e}")))?,
    );
    Ok((store, lock))
}

#[cfg(feature = "dynamodb")]
async fn create_dynamodb(config: &StateConfig) -> Result<StatePair, ServerError> {
    let dynamo_config = DynamoConfig {
        table_name: config
            .table_name
            .clone()
            .unwrap_or_else(|| "acteon_state".to_owned()),
        region: config
            .region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_owned()),
        endpoint_url: config.url.clone(),
        key_prefix: config.prefix.clone().unwrap_or_else(|| "acteon".to_owned()),
    };

    // When a custom endpoint is configured (DynamoDB Local) auto-create the
    // table so `docker compose --profile dynamodb up` works out of the box.
    if dynamo_config.endpoint_url.is_some() {
        let client = build_client(&dynamo_config).await;
        create_table(&client, &dynamo_config.table_name)
            .await
            .map_err(|e| ServerError::Config(format!("dynamodb create table: {e}")))?;
    }

    let store = Arc::new(
        DynamoStateStore::new(&dynamo_config)
            .await
            .map_err(|e| ServerError::Config(format!("dynamodb store: {e}")))?,
    );
    let lock = Arc::new(
        DynamoDistributedLock::new(&dynamo_config)
            .await
            .map_err(|e| ServerError::Config(format!("dynamodb lock: {e}")))?,
    );
    Ok((store, lock))
}

#[cfg(feature = "etcd")]
async fn create_etcd(config: &StateConfig) -> Result<StatePair, ServerError> {
    let endpoint = config
        .url
        .clone()
        .unwrap_or_else(|| "http://localhost:2379".to_owned());
    let etcd_config = EtcdConfig {
        endpoints: vec![endpoint],
        prefix: config.prefix.clone().unwrap_or_else(|| "acteon".to_owned()),
        ..EtcdConfig::default()
    };
    let store = Arc::new(
        EtcdStateStore::new(etcd_config.clone())
            .await
            .map_err(|e| ServerError::Config(format!("etcd store: {e}")))?,
    );
    let lock = Arc::new(
        EtcdDistributedLock::new(etcd_config)
            .await
            .map_err(|e| ServerError::Config(format!("etcd lock: {e}")))?,
    );
    Ok((store, lock))
}

#[cfg(feature = "clickhouse")]
async fn create_clickhouse(config: &StateConfig) -> Result<StatePair, ServerError> {
    let url = config.url.as_deref().unwrap_or("http://localhost:8123");
    let ch_config = ClickHouseConfig {
        url: url.to_owned(),
        table_prefix: config
            .prefix
            .clone()
            .unwrap_or_else(|| "acteon_".to_owned()),
        ..ClickHouseConfig::default()
    };
    let store = Arc::new(
        ClickHouseStateStore::new(ch_config.clone())
            .await
            .map_err(|e| ServerError::Config(format!("clickhouse store: {e}")))?,
    );
    let lock = Arc::new(
        ClickHouseDistributedLock::new(ch_config)
            .await
            .map_err(|e| ServerError::Config(format!("clickhouse lock: {e}")))?,
    );
    Ok((store, lock))
}
