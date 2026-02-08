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
    /// Actions allowed by the LLM guardrail.
    pub llm_guardrail_allowed: AtomicU64,
    /// Actions denied by the LLM guardrail.
    pub llm_guardrail_denied: AtomicU64,
    /// LLM guardrail evaluation errors.
    pub llm_guardrail_errors: AtomicU64,
    /// Task chains started.
    pub chains_started: AtomicU64,
    /// Task chains completed successfully.
    pub chains_completed: AtomicU64,
    /// Task chains failed.
    pub chains_failed: AtomicU64,
    /// Task chains cancelled.
    pub chains_cancelled: AtomicU64,
    /// Actions rejected because the provider circuit breaker was open.
    pub circuit_open: AtomicU64,
    /// Circuit breaker state transitions (any direction).
    pub circuit_transitions: AtomicU64,
    /// Actions rerouted to a fallback provider due to an open circuit.
    pub circuit_fallbacks: AtomicU64,
    /// Actions scheduled for delayed execution.
    pub scheduled: AtomicU64,
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

    /// Increment the LLM guardrail allowed counter.
    pub fn increment_llm_guardrail_allowed(&self) {
        self.llm_guardrail_allowed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the LLM guardrail denied counter.
    pub fn increment_llm_guardrail_denied(&self) {
        self.llm_guardrail_denied.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the LLM guardrail errors counter.
    pub fn increment_llm_guardrail_errors(&self) {
        self.llm_guardrail_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the chains started counter.
    pub fn increment_chains_started(&self) {
        self.chains_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the chains completed counter.
    pub fn increment_chains_completed(&self) {
        self.chains_completed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the chains failed counter.
    pub fn increment_chains_failed(&self) {
        self.chains_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the chains cancelled counter.
    pub fn increment_chains_cancelled(&self) {
        self.chains_cancelled.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the circuit-open rejection counter.
    pub fn increment_circuit_open(&self) {
        self.circuit_open.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the circuit state transition counter.
    pub fn increment_circuit_transitions(&self) {
        self.circuit_transitions.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the circuit fallback counter.
    pub fn increment_circuit_fallbacks(&self) {
        self.circuit_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the scheduled counter.
    pub fn increment_scheduled(&self) {
        self.scheduled.fetch_add(1, Ordering::Relaxed);
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
            llm_guardrail_allowed: self.llm_guardrail_allowed.load(Ordering::Relaxed),
            llm_guardrail_denied: self.llm_guardrail_denied.load(Ordering::Relaxed),
            llm_guardrail_errors: self.llm_guardrail_errors.load(Ordering::Relaxed),
            chains_started: self.chains_started.load(Ordering::Relaxed),
            chains_completed: self.chains_completed.load(Ordering::Relaxed),
            chains_failed: self.chains_failed.load(Ordering::Relaxed),
            chains_cancelled: self.chains_cancelled.load(Ordering::Relaxed),
            circuit_open: self.circuit_open.load(Ordering::Relaxed),
            circuit_transitions: self.circuit_transitions.load(Ordering::Relaxed),
            circuit_fallbacks: self.circuit_fallbacks.load(Ordering::Relaxed),
            scheduled: self.scheduled.load(Ordering::Relaxed),
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
    /// Actions allowed by the LLM guardrail.
    pub llm_guardrail_allowed: u64,
    /// Actions denied by the LLM guardrail.
    pub llm_guardrail_denied: u64,
    /// LLM guardrail evaluation errors.
    pub llm_guardrail_errors: u64,
    /// Task chains started.
    pub chains_started: u64,
    /// Task chains completed successfully.
    pub chains_completed: u64,
    /// Task chains failed.
    pub chains_failed: u64,
    /// Task chains cancelled.
    pub chains_cancelled: u64,
    /// Actions rejected because the provider circuit breaker was open.
    pub circuit_open: u64,
    /// Circuit breaker state transitions.
    pub circuit_transitions: u64,
    /// Actions rerouted to a fallback due to an open circuit.
    pub circuit_fallbacks: u64,
    /// Actions scheduled for delayed execution.
    pub scheduled: u64,
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
        assert_eq!(snap.llm_guardrail_allowed, 0);
        assert_eq!(snap.llm_guardrail_denied, 0);
        assert_eq!(snap.llm_guardrail_errors, 0);
        assert_eq!(snap.chains_started, 0);
        assert_eq!(snap.chains_completed, 0);
        assert_eq!(snap.chains_failed, 0);
        assert_eq!(snap.chains_cancelled, 0);
        assert_eq!(snap.circuit_open, 0);
        assert_eq!(snap.circuit_transitions, 0);
        assert_eq!(snap.circuit_fallbacks, 0);
        assert_eq!(snap.scheduled, 0);
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
        m.increment_llm_guardrail_allowed();
        m.increment_llm_guardrail_denied();
        m.increment_llm_guardrail_errors();
        m.increment_chains_started();
        m.increment_chains_completed();
        m.increment_chains_failed();
        m.increment_chains_cancelled();
        m.increment_circuit_open();
        m.increment_circuit_transitions();
        m.increment_circuit_fallbacks();
        m.increment_scheduled();

        let snap = m.snapshot();
        assert_eq!(snap.dispatched, 2);
        assert_eq!(snap.executed, 1);
        assert_eq!(snap.deduplicated, 1);
        assert_eq!(snap.suppressed, 1);
        assert_eq!(snap.rerouted, 1);
        assert_eq!(snap.throttled, 1);
        assert_eq!(snap.failed, 1);
        assert_eq!(snap.pending_approval, 1);
        assert_eq!(snap.llm_guardrail_allowed, 1);
        assert_eq!(snap.llm_guardrail_denied, 1);
        assert_eq!(snap.llm_guardrail_errors, 1);
        assert_eq!(snap.chains_started, 1);
        assert_eq!(snap.chains_completed, 1);
        assert_eq!(snap.chains_failed, 1);
        assert_eq!(snap.chains_cancelled, 1);
        assert_eq!(snap.circuit_open, 1);
        assert_eq!(snap.circuit_transitions, 1);
        assert_eq!(snap.circuit_fallbacks, 1);
        assert_eq!(snap.scheduled, 1);
    }
}
