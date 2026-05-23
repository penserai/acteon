use async_trait::async_trait;

use crate::types::SwarmRunSnapshot;

/// Hook invoked when a swarm run reaches a new terminal or lifecycle state.
///
/// The provider itself has no access to the audit store, stream broadcaster,
/// or gateway — consumers plug in their own sink so the completion can be
/// reported wherever makes sense. `acteon-server` supplies a sink that appends
/// an audit record and broadcasts a stream event; tests supply a no-op.
#[async_trait]
pub trait CompletionSink: Send + Sync + 'static {
    /// Called exactly once per state transition. Implementations must be fast
    /// and idempotent — the registry does not retry on error.
    async fn on_status(&self, snapshot: &SwarmRunSnapshot);
}

/// Sink that does nothing. Default for the CLI / tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSink;

#[async_trait]
impl CompletionSink for NoopSink {
    async fn on_status(&self, _snapshot: &SwarmRunSnapshot) {}
}

/// Sink that emits a `tracing` event at `info` on each transition.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoggingSink;

#[async_trait]
impl CompletionSink for LoggingSink {
    async fn on_status(&self, snapshot: &SwarmRunSnapshot) {
        tracing::info!(
            run_id = %snapshot.run_id,
            status = ?snapshot.status,
            namespace = %snapshot.namespace,
            tenant = %snapshot.tenant,
            objective = %snapshot.objective,
            "swarm run status",
        );
    }
}
