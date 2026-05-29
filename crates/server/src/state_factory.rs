use std::sync::Arc;
use std::time::Duration;

use acteon_state::{DistributedLock, StateStore};
#[cfg(feature = "dynamodb")]
use acteon_state_dynamodb::{
    DynamoConfig, DynamoDistributedLock, DynamoStateStore, build_client, create_table,
};
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
#[allow(clippy::unused_async)]
pub async fn create_state(config: &StateConfig) -> Result<StatePair, ServerError> {
    match config.backend.as_str() {
        "memory" => Ok(create_memory(config.memory_sweep_interval_secs)),
        #[cfg(feature = "redis")]
        "redis" => create_redis(config),
        #[cfg(feature = "postgres")]
        "postgres" => create_postgres(config).await,
        #[cfg(feature = "dynamodb")]
        "dynamodb" => create_dynamodb(config).await,
        other => Err(ServerError::Config(format!(
            "unsupported state backend: {other} (is the feature enabled?)"
        ))),
    }
}

fn create_memory(sweep_interval_secs: u64) -> StatePair {
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    // The memory backend evicts TTL'd entries lazily on read, so a
    // background sweeper is needed to reclaim entries that are never read
    // again. Other backends delegate expiry to the underlying store.
    if sweep_interval_secs > 0 {
        spawn_memory_sweeper(
            Arc::clone(&store),
            Arc::clone(&lock),
            Duration::from_secs(sweep_interval_secs),
        );
    }

    (store, lock)
}

/// Spawn a background task that periodically reclaims TTL-expired entries
/// and locks from the in-memory backend.
fn spawn_memory_sweeper(
    store: Arc<MemoryStateStore>,
    lock: Arc<MemoryDistributedLock>,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // The first tick fires immediately; skip it so the first real sweep
        // happens one interval in, by which point entries may have expired.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let entries = store.sweep_expired();
            let locks = lock.sweep_expired();
            if entries > 0 || locks > 0 {
                tracing::debug!(
                    entries,
                    locks,
                    "memory backend swept TTL-expired entries and locks"
                );
            }
        }
    });
}

#[cfg(feature = "redis")]
fn create_redis(config: &StateConfig) -> Result<StatePair, ServerError> {
    let url = config.url.as_deref().unwrap_or("redis://127.0.0.1:6379");
    let redis_config = RedisConfig {
        url: url.to_owned(),
        prefix: config.prefix.clone().unwrap_or_else(|| "acteon".to_owned()),
        tls_enabled: config.tls_enabled.unwrap_or(false),
        tls_insecure: config.tls_insecure.unwrap_or(false),
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
        ssl_mode: config.ssl_mode.clone(),
        ssl_root_cert: config.ssl_root_cert.clone(),
        ssl_cert: config.ssl_cert.clone(),
        ssl_key: config.ssl_key.clone(),
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
