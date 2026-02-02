use std::sync::Arc;

use acteon_audit::store::AuditStore;
use acteon_audit_clickhouse::{ClickHouseAuditConfig, ClickHouseAuditStore};
use acteon_audit_elasticsearch::{ElasticsearchAuditConfig, ElasticsearchAuditStore};
use acteon_audit_memory::MemoryAuditStore;
use acteon_audit_postgres::{PostgresAuditConfig, PostgresAuditStore};

use crate::config::AuditConfig;
use crate::error::ServerError;

/// Create an audit store from the given configuration.
pub async fn create_audit_store(config: &AuditConfig) -> Result<Arc<dyn AuditStore>, ServerError> {
    match config.backend.as_str() {
        "memory" => Ok(Arc::new(MemoryAuditStore::new())),
        "postgres" => {
            let url = config.url.as_deref().ok_or_else(|| {
                ServerError::Config("audit postgres backend requires [audit] url".into())
            })?;

            let pg_config = PostgresAuditConfig::new(url)
                .with_prefix(&config.prefix)
                .with_cleanup_interval(config.cleanup_interval_seconds);

            let store = PostgresAuditStore::new(&pg_config)
                .await
                .map_err(|e| ServerError::Config(format!("audit postgres: {e}")))?;

            Ok(Arc::new(store))
        }
        "clickhouse" => {
            let url = config.url.as_deref().ok_or_else(|| {
                ServerError::Config("audit clickhouse backend requires [audit] url".into())
            })?;

            let ch_config = ClickHouseAuditConfig::new(url)
                .with_prefix(&config.prefix)
                .with_cleanup_interval(config.cleanup_interval_seconds);

            let store = ClickHouseAuditStore::new(&ch_config)
                .await
                .map_err(|e| ServerError::Config(format!("audit clickhouse: {e}")))?;

            Ok(Arc::new(store))
        }
        "elasticsearch" => {
            let url = config.url.as_deref().ok_or_else(|| {
                ServerError::Config("audit elasticsearch backend requires [audit] url".into())
            })?;

            let es_config = ElasticsearchAuditConfig::new(url).with_index_prefix(&config.prefix);

            let store = ElasticsearchAuditStore::new(&es_config)
                .await
                .map_err(|e| ServerError::Config(format!("audit elasticsearch: {e}")))?;

            Ok(Arc::new(store))
        }
        other => Err(ServerError::Config(format!(
            "unknown audit backend: {other}"
        ))),
    }
}
