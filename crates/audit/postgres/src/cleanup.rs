use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use acteon_audit::store::AuditStore;

/// Spawn a background task that periodically cleans up expired audit records.
///
/// Returns a `JoinHandle` that can be used to abort the task on shutdown.
pub fn spawn_cleanup_task(
    store: Arc<dyn AuditStore>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        // The first tick completes immediately; skip it so we don't run cleanup
        // at startup.
        timer.tick().await;

        loop {
            timer.tick().await;
            match store.cleanup_expired().await {
                Ok(0) => {}
                Ok(n) => info!(removed = n, "audit cleanup removed expired records"),
                Err(e) => warn!(error = %e, "audit cleanup failed"),
            }
        }
    })
}
