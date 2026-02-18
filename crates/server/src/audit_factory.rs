use std::sync::Arc;

use acteon_audit::store::AuditStore;
use acteon_audit::{RedactConfig, RedactingAuditStore};
#[cfg(feature = "clickhouse")]
use acteon_audit_clickhouse::{ClickHouseAuditConfig, ClickHouseAuditStore};
#[cfg(feature = "dynamodb")]
use acteon_audit_dynamodb::{DynamoDbAuditConfig, DynamoDbAuditStore};
#[cfg(feature = "elasticsearch")]
use acteon_audit_elasticsearch::{ElasticsearchAuditConfig, ElasticsearchAuditStore};
use acteon_audit_memory::MemoryAuditStore;
#[cfg(feature = "postgres")]
use acteon_audit_postgres::{PostgresAuditConfig, PostgresAuditStore};

use crate::config::AuditConfig;
use crate::error::ServerError;

/// Create an audit store from the given configuration.
#[allow(clippy::unused_async)]
pub async fn create_audit_store(config: &AuditConfig) -> Result<Arc<dyn AuditStore>, ServerError> {
    let store: Arc<dyn AuditStore> = match config.backend.as_str() {
        "memory" => Arc::new(MemoryAuditStore::new()),
        #[cfg(feature = "postgres")]
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

            Arc::new(store)
        }
        #[cfg(feature = "clickhouse")]
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

            Arc::new(store)
        }
        #[cfg(feature = "dynamodb")]
        "dynamodb" => {
            let dynamo_config = DynamoDbAuditConfig {
                table_name: config
                    .table_name
                    .clone()
                    .unwrap_or_else(|| "acteon_audit".to_owned()),
                region: config
                    .region
                    .clone()
                    .unwrap_or_else(|| "us-east-1".to_owned()),
                endpoint_url: config.url.clone(),
                key_prefix: config.prefix.clone(),
            };

            // Auto-create table in dev mode (when endpoint_url is set).
            if dynamo_config.endpoint_url.is_some() {
                let client = acteon_audit_dynamodb::build_client(&dynamo_config).await;
                acteon_audit_dynamodb::create_audit_table(&client, &dynamo_config.table_name)
                    .await
                    .map_err(|e| {
                        ServerError::Config(format!("audit dynamodb table creation: {e}"))
                    })?;
            }

            Arc::new(DynamoDbAuditStore::new(&dynamo_config).await)
        }
        #[cfg(feature = "elasticsearch")]
        "elasticsearch" => {
            let url = config.url.as_deref().ok_or_else(|| {
                ServerError::Config("audit elasticsearch backend requires [audit] url".into())
            })?;

            let es_config = ElasticsearchAuditConfig::new(url).with_index_prefix(&config.prefix);

            let store = ElasticsearchAuditStore::new(&es_config)
                .await
                .map_err(|e| ServerError::Config(format!("audit elasticsearch: {e}")))?;

            Arc::new(store)
        }
        other => {
            return Err(ServerError::Config(format!(
                "unknown audit backend: {other} (is the feature enabled?)"
            )));
        }
    };

    // Wrap with redaction if enabled.
    if config.redact.enabled && !config.redact.fields.is_empty() {
        let redact_config = RedactConfig {
            fields: config.redact.fields.clone(),
            placeholder: config.redact.placeholder.clone(),
        };
        Ok(Arc::new(RedactingAuditStore::new(store, &redact_config)))
    } else {
        Ok(store)
    }
}
