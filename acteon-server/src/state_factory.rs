use std::sync::Arc;

use acteon_state::{DistributedLock, StateStore};
use acteon_state_dynamodb::{DynamoConfig, DynamoDistributedLock, DynamoStateStore};
use acteon_state_etcd::{EtcdConfig, EtcdDistributedLock, EtcdStateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_state_postgres::{PostgresConfig, PostgresDistributedLock, PostgresStateStore};
use acteon_state_redis::{RedisConfig, RedisDistributedLock, RedisStateStore};

use crate::config::StateConfig;
use crate::error::ServerError;

/// A state store and distributed lock pair.
pub type StatePair = (Arc<dyn StateStore>, Arc<dyn DistributedLock>);

/// Construct a `StateStore` and `DistributedLock` pair from configuration.
pub async fn create_state(config: &StateConfig) -> Result<StatePair, ServerError> {
    match config.backend.as_str() {
        "memory" => {
            let store = Arc::new(MemoryStateStore::new());
            let lock = Arc::new(MemoryDistributedLock::new());
            Ok((store, lock))
        }
        "redis" => {
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
        "postgres" => {
            let url = config.url.as_deref().ok_or_else(|| {
                ServerError::Config("postgres backend requires 'url' in [state]".into())
            })?;
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
        "dynamodb" => {
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
        "etcd" => {
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
        other => Err(ServerError::Config(format!(
            "unsupported state backend: {other}"
        ))),
    }
}
