use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counters tracking gateway dispatch outcomes.
///
/// All counters use relaxed ordering for maximum throughput. For a
/// consistent point-in-time view, call [`snapshot`](Self::snapshot).
#[derive(Debug, Default)]
pub struct GatewayMetrics {
    /// Total number of actions dispatched.
    pub dispatched: AtomicU64,
    /// Actions that were successfully executed by a provider.
    pub executed: AtomicU64,
    /// Actions that were deduplicated (skipped as already processed).
    pub deduplicated: AtomicU64,
    /// Actions that were suppressed by a rule.
    pub suppressed: AtomicU64,
    /// Actions that were rerouted to a different provider.
    pub rerouted: AtomicU64,
    /// Actions that were throttled.
    pub throttled: AtomicU64,
    /// Actions that failed after all retries.
    pub failed: AtomicU64,
    /// Actions pending human approval.
    pub pending_approval: AtomicU64,
}

impl GatewayMetrics {
    /// Increment the dispatched counter.
    pub fn increment_dispatched(&self) {
        self.dispatched.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the executed counter.
    pub fn increment_executed(&self) {
        self.executed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the deduplicated counter.
    pub fn increment_deduplicated(&self) {
        self.deduplicated.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the suppressed counter.
    pub fn increment_suppressed(&self) {
        self.suppressed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the rerouted counter.
    pub fn increment_rerouted(&self) {
        self.rerouted.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the throttled counter.
    pub fn increment_throttled(&self) {
        self.throttled.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the failed counter.
    pub fn increment_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the pending approval counter.
    pub fn increment_pending_approval(&self) {
        self.pending_approval.fetch_add(1, Ordering::Relaxed);
    }

    /// Take a consistent point-in-time snapshot of all counters.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            dispatched: self.dispatched.load(Ordering::Relaxed),
            executed: self.executed.load(Ordering::Relaxed),
            deduplicated: self.deduplicated.load(Ordering::Relaxed),
            suppressed: self.suppressed.load(Ordering::Relaxed),
            rerouted: self.rerouted.load(Ordering::Relaxed),
            throttled: self.throttled.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            pending_approval: self.pending_approval.load(Ordering::Relaxed),
        }
    }
}

/// A plain data snapshot of [`GatewayMetrics`] at a point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Total number of actions dispatched.
    pub dispatched: u64,
    /// Actions that were successfully executed by a provider.
    pub executed: u64,
    /// Actions that were deduplicated.
    pub deduplicated: u64,
    /// Actions that were suppressed by a rule.
    pub suppressed: u64,
    /// Actions that were rerouted to a different provider.
    pub rerouted: u64,
    /// Actions that were throttled.
    pub throttled: u64,
    /// Actions that failed.
    pub failed: u64,
    /// Actions pending human approval.
    pub pending_approval: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metrics_are_zero() {
        let m = GatewayMetrics::default();
        let snap = m.snapshot();
        assert_eq!(snap.dispatched, 0);
        assert_eq!(snap.executed, 0);
        assert_eq!(snap.deduplicated, 0);
        assert_eq!(snap.suppressed, 0);
        assert_eq!(snap.rerouted, 0);
        assert_eq!(snap.throttled, 0);
        assert_eq!(snap.failed, 0);
        assert_eq!(snap.pending_approval, 0);
    }

    #[test]
    fn increment_and_snapshot() {
        let m = GatewayMetrics::default();
        m.increment_dispatched();
        m.increment_dispatched();
        m.increment_executed();
        m.increment_deduplicated();
        m.increment_suppressed();
        m.increment_rerouted();
        m.increment_throttled();
        m.increment_failed();
        m.increment_pending_approval();

        let snap = m.snapshot();
        assert_eq!(snap.dispatched, 2);
        assert_eq!(snap.executed, 1);
        assert_eq!(snap.deduplicated, 1);
        assert_eq!(snap.suppressed, 1);
        assert_eq!(snap.rerouted, 1);
        assert_eq!(snap.throttled, 1);
        assert_eq!(snap.failed, 1);
        assert_eq!(snap.pending_approval, 1);
    }
}
