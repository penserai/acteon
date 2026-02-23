use std::sync::Arc;

use acteon_audit::AnalyticsStore;
use acteon_audit::store::AuditStore;

/// Create an analytics store from an audit store.
///
/// If the audit store (or a backend wrapped beneath decorators) provides a
/// native analytics implementation, that is used. Otherwise falls back to the
/// generic `InMemoryAnalytics` engine which works with any `AuditStore`.
#[allow(clippy::unused_async)]
pub async fn create_analytics_store(
    audit_store: &Arc<dyn AuditStore>,
) -> Option<Arc<dyn AnalyticsStore>> {
    if let Some(native) = audit_store.analytics() {
        return Some(native);
    }
    Some(Arc::new(acteon_audit::InMemoryAnalytics::new(Arc::clone(
        audit_store,
    ))))
}
