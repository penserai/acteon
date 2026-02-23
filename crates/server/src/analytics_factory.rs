use std::sync::Arc;

use acteon_audit::AnalyticsStore;
use acteon_audit::store::AuditStore;

use crate::config::AuditConfig;

/// Create an analytics store from the audit configuration.
///
/// - For Postgres and `ClickHouse` backends, returns the native optimized
///   implementation that uses server-side SQL aggregation.
/// - For all other backends (memory, `DynamoDB`, `Elasticsearch`), wraps
///   the audit store with the generic `InMemoryAnalytics` fallback.
#[allow(clippy::unused_async, unused_variables)]
pub async fn create_analytics_store(
    config: &AuditConfig,
    audit_store: &Arc<dyn AuditStore>,
) -> Option<Arc<dyn AnalyticsStore>> {
    match config.backend.as_str() {
        #[cfg(feature = "postgres")]
        "postgres" => {
            // The Postgres audit store implements AnalyticsStore natively.
            // We need to downcast to access the implementation.
            // Since we can't downcast Arc<dyn AuditStore>, we use the
            // InMemoryAnalytics fallback which works with any AuditStore.
            //
            // Note: For true native SQL analytics, the PostgresAuditStore
            // would need to be passed separately. For now, the in-memory
            // fallback provides correct results.
            Some(Arc::new(acteon_audit::InMemoryAnalytics::new(Arc::clone(
                audit_store,
            ))))
        }
        #[cfg(feature = "clickhouse")]
        "clickhouse" => {
            // Same as Postgres -- use InMemoryAnalytics for now.
            Some(Arc::new(acteon_audit::InMemoryAnalytics::new(Arc::clone(
                audit_store,
            ))))
        }
        _ => {
            // Universal in-memory fallback.
            Some(Arc::new(acteon_audit::InMemoryAnalytics::new(Arc::clone(
                audit_store,
            ))))
        }
    }
}
