use std::collections::{HashMap, VecDeque};
use std::sync::{
    LazyLock,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

use regex::Regex;

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
    /// Recurring actions dispatched successfully.
    pub recurring_dispatched: AtomicU64,
    /// Recurring action dispatch errors.
    pub recurring_errors: AtomicU64,
    /// Recurring actions skipped (disabled, expired, etc.).
    pub recurring_skipped: AtomicU64,
    /// Actions blocked by tenant quota.
    pub quota_exceeded: AtomicU64,
    /// Actions that passed with a quota warning.
    pub quota_warned: AtomicU64,
    /// Actions degraded to a fallback provider due to quota.
    pub quota_degraded: AtomicU64,
    /// Actions that triggered a quota notification.
    pub quota_notified: AtomicU64,
    /// State entries deleted by the retention reaper.
    pub retention_deleted_state: AtomicU64,
    /// Retention reaper skipped entries due to compliance hold.
    pub retention_skipped_compliance: AtomicU64,
    /// Retention reaper errors.
    pub retention_errors: AtomicU64,
    /// WASM plugin invocations.
    pub wasm_invocations: AtomicU64,
    /// WASM plugin invocation errors.
    pub wasm_errors: AtomicU64,
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

    /// Increment the recurring dispatched counter.
    pub fn increment_recurring_dispatched(&self) {
        self.recurring_dispatched.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the recurring errors counter.
    pub fn increment_recurring_errors(&self) {
        self.recurring_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the recurring skipped counter.
    pub fn increment_recurring_skipped(&self) {
        self.recurring_skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the quota exceeded (blocked) counter.
    pub fn increment_quota_exceeded(&self) {
        self.quota_exceeded.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the quota warned counter.
    pub fn increment_quota_warned(&self) {
        self.quota_warned.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the quota degraded counter.
    pub fn increment_quota_degraded(&self) {
        self.quota_degraded.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the quota notified counter.
    pub fn increment_quota_notified(&self) {
        self.quota_notified.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the retention deleted state counter.
    pub fn increment_retention_deleted_state(&self) {
        self.retention_deleted_state.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the retention skipped compliance counter.
    pub fn increment_retention_skipped_compliance(&self) {
        self.retention_skipped_compliance
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the retention errors counter.
    pub fn increment_retention_errors(&self) {
        self.retention_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment the WASM invocations counter.
    pub fn increment_wasm_invocations(&self) {
        self.wasm_invocations.fetch_add(1, Ordering::Relaxed);
    }

    /// Add `n` to the WASM invocations counter.
    pub fn add_wasm_invocations(&self, n: u64) {
        self.wasm_invocations.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment the WASM errors counter.
    pub fn increment_wasm_errors(&self) {
        self.wasm_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Add `n` to the WASM errors counter.
    pub fn add_wasm_errors(&self, n: u64) {
        self.wasm_errors.fetch_add(n, Ordering::Relaxed);
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
            recurring_dispatched: self.recurring_dispatched.load(Ordering::Relaxed),
            recurring_errors: self.recurring_errors.load(Ordering::Relaxed),
            recurring_skipped: self.recurring_skipped.load(Ordering::Relaxed),
            quota_exceeded: self.quota_exceeded.load(Ordering::Relaxed),
            quota_warned: self.quota_warned.load(Ordering::Relaxed),
            quota_degraded: self.quota_degraded.load(Ordering::Relaxed),
            quota_notified: self.quota_notified.load(Ordering::Relaxed),
            retention_deleted_state: self.retention_deleted_state.load(Ordering::Relaxed),
            retention_skipped_compliance: self.retention_skipped_compliance.load(Ordering::Relaxed),
            retention_errors: self.retention_errors.load(Ordering::Relaxed),
            wasm_invocations: self.wasm_invocations.load(Ordering::Relaxed),
            wasm_errors: self.wasm_errors.load(Ordering::Relaxed),
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
    /// Recurring actions dispatched successfully.
    pub recurring_dispatched: u64,
    /// Recurring action dispatch errors.
    pub recurring_errors: u64,
    /// Recurring actions skipped (disabled, expired, etc.).
    pub recurring_skipped: u64,
    /// Actions blocked by tenant quota.
    pub quota_exceeded: u64,
    /// Actions that passed with a quota warning.
    pub quota_warned: u64,
    /// Actions degraded to a fallback provider due to quota.
    pub quota_degraded: u64,
    /// Actions that triggered a quota notification.
    pub quota_notified: u64,
    /// State entries deleted by the retention reaper.
    pub retention_deleted_state: u64,
    /// Retention reaper skipped entries due to compliance hold.
    pub retention_skipped_compliance: u64,
    /// Retention reaper errors.
    pub retention_errors: u64,
    /// WASM plugin invocations.
    pub wasm_invocations: u64,
    /// WASM plugin invocation errors.
    pub wasm_errors: u64,
}

/// Maximum number of latency samples retained per provider.
///
/// When the buffer is full the oldest sample is evicted. 1 000 samples gives
/// accurate p99 values for low-to-medium traffic providers (< 100 req/s) while
/// consuming ~8 KB per provider.
///
/// **Limitation for high-traffic providers**: For providers handling 1000+ req/s,
/// percentiles will only represent the most recent ~1 second of traffic and may
/// not reflect long-term performance characteristics. For production-grade metrics
/// and historical analysis, export to Prometheus or a similar metrics backend.
const MAX_LATENCY_SAMPLES: usize = 1_000;

/// Maximum length of error messages stored in `last_error`.
///
/// Long error messages (stack traces, large responses) are truncated to prevent
/// unbounded memory growth and potential information leakage of sensitive details
/// like internal URLs, credentials, or file paths.
const MAX_ERROR_MESSAGE_LEN: usize = 500;

/// Maximum number of unique providers tracked by `ProviderMetrics`.
///
/// Prevents unbounded memory growth from malicious or misconfigured clients
/// creating unlimited unique provider names. Once limit is reached, new providers
/// are ignored for metrics tracking (but requests are still processed normally).
const MAX_TRACKED_PROVIDERS: usize = 1_000;

// Compiled regex patterns for error sanitization (compiled once, used many times).
static URL_CREDS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(https?://)[^:/@]+:[^@]+@").expect("valid regex"));
static AUTH_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization|api[-_]?key|bearer|token)[\s:=]+(?:bearer\s+)?[^\s,}]+")
        .expect("valid regex")
});
static FILE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(/[a-zA-Z][a-zA-Z0-9_\-./]*|[A-Z]:\\[a-zA-Z0-9_\-\\/.]+)").expect("valid regex")
});

/// Sanitize an error message to prevent information leakage.
///
/// Truncates long messages and redacts patterns that may contain sensitive data:
/// - URLs with embedded credentials (e.g., `https://user:pass@host`)
/// - API keys and tokens (common patterns like `Bearer`, `API-Key`, `token=`)
/// - File paths (absolute paths starting with `/` or `C:\`)
fn sanitize_error_message(msg: &str) -> String {
    // Truncate to max length first
    let truncated = if msg.len() > MAX_ERROR_MESSAGE_LEN {
        format!("{}... (truncated)", &msg[..MAX_ERROR_MESSAGE_LEN])
    } else {
        msg.to_owned()
    };

    // Redact URLs with embedded credentials (basic auth)
    let mut sanitized = URL_CREDS_RE
        .replace_all(&truncated, "${1}***:***@")
        .to_string();

    // Redact common auth header patterns
    sanitized = AUTH_HEADER_RE
        .replace_all(&sanitized, "${1}: ***")
        .to_string();

    // Redact absolute file paths (both Unix and Windows)
    sanitized = FILE_PATH_RE
        .replace_all(&sanitized, "[PATH_REDACTED]")
        .to_string();

    sanitized
}

/// Per-provider execution statistics.
///
/// All atomic fields use relaxed ordering for throughput. The latency sample
/// buffer and last-error string are guarded by a `parking_lot::Mutex` which
/// is held only for short, non-blocking durations.
pub struct ProviderStats {
    total_requests: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    /// Cumulative latency in microseconds (for average calculation).
    total_latency_us: AtomicU64,
    /// Rolling window of individual latency samples (microseconds).
    latency_samples: parking_lot::Mutex<VecDeque<u64>>,
    /// Unix-millisecond timestamp of the most recent request.
    last_request_at: AtomicI64,
    /// Most recent error message.
    last_error: parking_lot::Mutex<Option<String>>,
}

impl std::fmt::Debug for ProviderStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderStats")
            .field(
                "total_requests",
                &self.total_requests.load(Ordering::Relaxed),
            )
            .field("successes", &self.successes.load(Ordering::Relaxed))
            .field("failures", &self.failures.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Default for ProviderStats {
    fn default() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            total_latency_us: AtomicU64::new(0),
            latency_samples: parking_lot::Mutex::new(VecDeque::with_capacity(MAX_LATENCY_SAMPLES)),
            last_request_at: AtomicI64::new(0),
            last_error: parking_lot::Mutex::new(None),
        }
    }
}

impl ProviderStats {
    /// Record a successful execution.
    pub fn record_success(&self, latency_us: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successes.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency_us, Ordering::Relaxed);
        self.push_latency(latency_us);
        self.touch();
    }

    /// Record a failed execution.
    pub fn record_failure(&self, latency_us: u64, error: &str) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failures.fetch_add(1, Ordering::Relaxed);
        self.total_latency_us
            .fetch_add(latency_us, Ordering::Relaxed);
        self.push_latency(latency_us);
        self.touch();
        // Sanitize error message to prevent information leakage
        *self.last_error.lock() = Some(sanitize_error_message(error));
    }

    fn push_latency(&self, us: u64) {
        let mut buf = self.latency_samples.lock();
        if buf.len() >= MAX_LATENCY_SAMPLES {
            buf.pop_front();
        }
        buf.push_back(us);
    }

    fn touch(&self) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.last_request_at.store(now_ms, Ordering::Relaxed);
    }

    /// Take a point-in-time snapshot.
    #[allow(clippy::cast_precision_loss)]
    pub fn snapshot(&self) -> ProviderStatsSnapshot {
        let total = self.total_requests.load(Ordering::Relaxed);
        let successes = self.successes.load(Ordering::Relaxed);
        let failures = self.failures.load(Ordering::Relaxed);
        let total_latency_us = self.total_latency_us.load(Ordering::Relaxed);
        let last_at = self.last_request_at.load(Ordering::Relaxed);

        let success_rate = if total > 0 {
            (successes as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let avg_latency_ms = if total > 0 {
            (total_latency_us as f64 / total as f64) / 1_000.0
        } else {
            0.0
        };

        let (p50, p95, p99) = self.compute_percentiles();

        ProviderStatsSnapshot {
            total_requests: total,
            successes,
            failures,
            success_rate,
            avg_latency_ms,
            p50_latency_ms: p50,
            p95_latency_ms: p95,
            p99_latency_ms: p99,
            last_request_at: if last_at > 0 { Some(last_at) } else { None },
            last_error: self.last_error.lock().clone(),
        }
    }

    fn compute_percentiles(&self) -> (f64, f64, f64) {
        let buf = self.latency_samples.lock();
        if buf.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let mut sorted: Vec<u64> = buf.iter().copied().collect();
        sorted.sort_unstable();
        let len = sorted.len();
        let p50 = percentile_value(&sorted, len, 50.0);
        let p95 = percentile_value(&sorted, len, 95.0);
        let p99 = percentile_value(&sorted, len, 99.0);
        (p50, p95, p99)
    }
}

/// Compute a percentile value from a sorted slice, converting microseconds to milliseconds.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn percentile_value(sorted: &[u64], len: usize, pct: f64) -> f64 {
    let idx = ((pct / 100.0) * (len as f64 - 1.0)).round() as usize;
    let idx = idx.min(len - 1);
    sorted[idx] as f64 / 1_000.0
}

/// Point-in-time snapshot of a single provider's stats.
#[derive(Debug, Clone)]
pub struct ProviderStatsSnapshot {
    pub total_requests: u64,
    pub successes: u64,
    pub failures: u64,
    /// Success rate as a percentage (0.0 - 100.0).
    pub success_rate: f64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
    /// p50 latency in milliseconds.
    pub p50_latency_ms: f64,
    /// p95 latency in milliseconds.
    pub p95_latency_ms: f64,
    /// p99 latency in milliseconds.
    pub p99_latency_ms: f64,
    /// Unix-millisecond timestamp of the last request.
    pub last_request_at: Option<i64>,
    /// Most recent error message.
    pub last_error: Option<String>,
}

/// Thread-safe container for per-provider execution metrics.
///
/// Providers are lazily registered on first use, so there is no setup
/// required beyond constructing the container.
///
/// ## In-Memory Metrics
///
/// All metrics are stored **in-memory** and will be **reset to zero** when the
/// gateway restarts. For historical analysis and production monitoring, export
/// metrics to Prometheus, Grafana, or a similar observability backend.
///
/// Stats are tracked **globally per provider** (not per-namespace or per-tenant).
/// For granular per-tenant analysis, use the audit log or Prometheus labels.
#[derive(Debug, Default)]
pub struct ProviderMetrics {
    providers: parking_lot::RwLock<HashMap<String, ProviderStats>>,
}

impl ProviderMetrics {
    /// Record a successful execution for `provider`.
    pub fn record_success(&self, provider: &str, latency_us: u64) {
        let map = self.providers.read();
        if let Some(stats) = map.get(provider) {
            stats.record_success(latency_us);
            return;
        }
        drop(map);
        // Upgrade to write lock and insert on first use.
        let mut map = self.providers.write();
        // Enforce maximum provider limit to prevent unbounded memory growth
        if !map.contains_key(provider) && map.len() >= MAX_TRACKED_PROVIDERS {
            // Silently drop metrics for new providers once limit is reached.
            // The action will still be processed normally, just no metrics tracked.
            return;
        }
        let stats = map.entry(provider.to_owned()).or_default();
        stats.record_success(latency_us);
    }

    /// Record a failed execution for `provider`.
    pub fn record_failure(&self, provider: &str, latency_us: u64, error: &str) {
        let map = self.providers.read();
        if let Some(stats) = map.get(provider) {
            stats.record_failure(latency_us, error);
            return;
        }
        drop(map);
        let mut map = self.providers.write();
        // Enforce maximum provider limit to prevent unbounded memory growth
        if !map.contains_key(provider) && map.len() >= MAX_TRACKED_PROVIDERS {
            // Silently drop metrics for new providers once limit is reached.
            // The action will still be processed normally, just no metrics tracked.
            return;
        }
        let stats = map.entry(provider.to_owned()).or_default();
        stats.record_failure(latency_us, error);
    }

    /// Take a snapshot of all providers.
    pub fn snapshot(&self) -> HashMap<String, ProviderStatsSnapshot> {
        let map = self.providers.read();
        map.iter()
            .map(|(name, stats)| (name.clone(), stats.snapshot()))
            .collect()
    }

    /// Take a snapshot for a single provider (returns `None` if never seen).
    pub fn snapshot_for(&self, provider: &str) -> Option<ProviderStatsSnapshot> {
        let map = self.providers.read();
        map.get(provider).map(ProviderStats::snapshot)
    }
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
        assert_eq!(snap.recurring_dispatched, 0);
        assert_eq!(snap.recurring_errors, 0);
        assert_eq!(snap.recurring_skipped, 0);
        assert_eq!(snap.quota_exceeded, 0);
        assert_eq!(snap.quota_warned, 0);
        assert_eq!(snap.quota_degraded, 0);
        assert_eq!(snap.quota_notified, 0);
        assert_eq!(snap.retention_deleted_state, 0);
        assert_eq!(snap.retention_skipped_compliance, 0);
        assert_eq!(snap.retention_errors, 0);
        assert_eq!(snap.wasm_invocations, 0);
        assert_eq!(snap.wasm_errors, 0);
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
        m.increment_recurring_dispatched();
        m.increment_recurring_errors();
        m.increment_recurring_skipped();
        m.increment_quota_exceeded();
        m.increment_quota_warned();
        m.increment_quota_degraded();
        m.increment_quota_notified();
        m.increment_retention_deleted_state();
        m.increment_retention_skipped_compliance();
        m.increment_retention_errors();
        m.increment_wasm_invocations();
        m.increment_wasm_errors();

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
        assert_eq!(snap.recurring_dispatched, 1);
        assert_eq!(snap.recurring_errors, 1);
        assert_eq!(snap.recurring_skipped, 1);
        assert_eq!(snap.quota_exceeded, 1);
        assert_eq!(snap.quota_warned, 1);
        assert_eq!(snap.quota_degraded, 1);
        assert_eq!(snap.quota_notified, 1);
        assert_eq!(snap.retention_deleted_state, 1);
        assert_eq!(snap.retention_skipped_compliance, 1);
        assert_eq!(snap.retention_errors, 1);
        assert_eq!(snap.wasm_invocations, 1);
        assert_eq!(snap.wasm_errors, 1);
    }

    #[test]
    fn provider_metrics_empty_snapshot() {
        let pm = ProviderMetrics::default();
        let snap = pm.snapshot();
        assert!(snap.is_empty());
    }

    #[test]
    fn provider_metrics_record_success() {
        let pm = ProviderMetrics::default();
        pm.record_success("email", 10_000); // 10ms
        pm.record_success("email", 20_000); // 20ms

        let snap = pm.snapshot_for("email").unwrap();
        assert_eq!(snap.total_requests, 2);
        assert_eq!(snap.successes, 2);
        assert_eq!(snap.failures, 0);
        assert!((snap.success_rate - 100.0).abs() < f64::EPSILON);
        assert!((snap.avg_latency_ms - 15.0).abs() < f64::EPSILON);
        assert!(snap.last_request_at.is_some());
        assert!(snap.last_error.is_none());
    }

    #[test]
    fn provider_metrics_record_failure() {
        let pm = ProviderMetrics::default();
        pm.record_success("slack", 5_000);
        pm.record_failure("slack", 100_000, "connection timeout");

        let snap = pm.snapshot_for("slack").unwrap();
        assert_eq!(snap.total_requests, 2);
        assert_eq!(snap.successes, 1);
        assert_eq!(snap.failures, 1);
        assert!((snap.success_rate - 50.0).abs() < f64::EPSILON);
        assert_eq!(snap.last_error.as_deref(), Some("connection timeout"));
    }

    #[test]
    fn provider_metrics_percentiles() {
        let pm = ProviderMetrics::default();
        // Insert 100 samples: 1ms, 2ms, ..., 100ms
        for i in 1..=100 {
            pm.record_success("webhook", i * 1_000);
        }

        let snap = pm.snapshot_for("webhook").unwrap();
        // p50 should be around 50ms
        assert!((snap.p50_latency_ms - 50.0).abs() < 2.0);
        // p95 should be around 95ms
        assert!((snap.p95_latency_ms - 95.0).abs() < 2.0);
        // p99 should be around 99ms
        assert!((snap.p99_latency_ms - 99.0).abs() < 2.0);
    }

    #[test]
    fn provider_metrics_multiple_providers() {
        let pm = ProviderMetrics::default();
        pm.record_success("email", 10_000);
        pm.record_success("slack", 20_000);
        pm.record_failure("pagerduty", 30_000, "auth error");

        let all = pm.snapshot();
        assert_eq!(all.len(), 3);
        assert!(all.contains_key("email"));
        assert!(all.contains_key("slack"));
        assert!(all.contains_key("pagerduty"));
    }

    #[test]
    fn provider_metrics_bounded_buffer() {
        let pm = ProviderMetrics::default();
        // Insert more than MAX_LATENCY_SAMPLES
        for i in 0..(MAX_LATENCY_SAMPLES + 500) {
            pm.record_success("test", (i as u64) * 100);
        }
        let snap = pm.snapshot_for("test").unwrap();
        assert_eq!(snap.total_requests, (MAX_LATENCY_SAMPLES + 500) as u64);
        // Percentiles should still be computable
        assert!(snap.p50_latency_ms > 0.0);
    }

    #[test]
    fn provider_metrics_unknown_provider() {
        let pm = ProviderMetrics::default();
        assert!(pm.snapshot_for("nonexistent").is_none());
    }

    // -- Additional comprehensive tests ---------------------------------------

    #[test]
    fn provider_metrics_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let pm = Arc::new(ProviderMetrics::default());
        let num_threads = 10;
        let ops_per_thread = 1_000;

        let mut handles = Vec::new();
        for thread_id in 0..num_threads {
            let pm_clone = Arc::clone(&pm);
            let handle = thread::spawn(move || {
                for i in 0..ops_per_thread {
                    if (thread_id + i) % 2 == 0 {
                        pm_clone.record_success("concurrent_test", 5_000);
                    } else {
                        pm_clone.record_failure("concurrent_test", 10_000, "test error");
                    }
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        let snap = pm.snapshot_for("concurrent_test").unwrap();
        assert_eq!(snap.total_requests, (num_threads * ops_per_thread) as u64);
        // Verify success/failure counts sum to total
        assert_eq!(snap.successes + snap.failures, snap.total_requests);
    }

    #[test]
    fn provider_metrics_percentiles_single_sample() {
        let pm = ProviderMetrics::default();
        pm.record_success("single", 42_000); // 42ms

        let snap = pm.snapshot_for("single").unwrap();
        assert!((snap.p50_latency_ms - 42.0).abs() < f64::EPSILON);
        assert!((snap.p95_latency_ms - 42.0).abs() < f64::EPSILON);
        assert!((snap.p99_latency_ms - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn provider_metrics_percentiles_two_samples() {
        let pm = ProviderMetrics::default();
        pm.record_success("two", 10_000); // 10ms
        pm.record_success("two", 20_000); // 20ms

        let snap = pm.snapshot_for("two").unwrap();
        // With two samples, percentiles should be one of the values
        assert!(
            (snap.p50_latency_ms - 10.0).abs() < 1.0 || (snap.p50_latency_ms - 20.0).abs() < 1.0
        );
        assert!(
            (snap.p99_latency_ms - 10.0).abs() < 1.0 || (snap.p99_latency_ms - 20.0).abs() < 1.0
        );
    }

    #[test]
    fn provider_metrics_percentiles_all_same() {
        let pm = ProviderMetrics::default();
        for _ in 0..100 {
            pm.record_success("same", 100_000); // 100ms every time
        }

        let snap = pm.snapshot_for("same").unwrap();
        assert!((snap.p50_latency_ms - 100.0).abs() < f64::EPSILON);
        assert!((snap.p95_latency_ms - 100.0).abs() < f64::EPSILON);
        assert!((snap.p99_latency_ms - 100.0).abs() < f64::EPSILON);
        assert!((snap.avg_latency_ms - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn provider_metrics_success_rate_precision() {
        let pm = ProviderMetrics::default();
        // Simulate millions of requests
        let total = 10_000_000_u64;
        let successes = 9_999_999_u64;

        // Record by directly manipulating stats (to avoid actually recording millions)
        pm.record_success("precision", 1_000);
        let map = pm.providers.read();
        let stats = map.get("precision").unwrap();

        // Manually set the counters for large numbers
        stats
            .total_requests
            .store(total, std::sync::atomic::Ordering::Relaxed);
        stats
            .successes
            .store(successes, std::sync::atomic::Ordering::Relaxed);
        stats
            .failures
            .store(total - successes, std::sync::atomic::Ordering::Relaxed);
        drop(map);

        let snap = pm.snapshot_for("precision").unwrap();
        #[allow(clippy::cast_precision_loss)]
        let expected_rate = (successes as f64 / total as f64) * 100.0;
        assert!((snap.success_rate - expected_rate).abs() < 0.001);
    }

    #[test]
    fn provider_metrics_last_error_overwrite() {
        let pm = ProviderMetrics::default();
        pm.record_failure("errors", 1_000, "first error");
        pm.record_failure("errors", 2_000, "second error");
        pm.record_failure("errors", 3_000, "third error");

        let snap = pm.snapshot_for("errors").unwrap();
        // Last error should be the most recent
        assert_eq!(snap.last_error.as_deref(), Some("third error"));
        assert_eq!(snap.failures, 3);
    }

    #[test]
    fn provider_metrics_snapshot_isolation() {
        let pm = ProviderMetrics::default();
        pm.record_success("isolation", 10_000);
        pm.record_success("isolation", 20_000);

        // Take a snapshot
        let snap1 = pm.snapshot_for("isolation").unwrap();
        assert_eq!(snap1.total_requests, 2);
        assert_eq!(snap1.successes, 2);

        // Record more data after snapshot
        pm.record_success("isolation", 30_000);
        pm.record_failure("isolation", 40_000, "oops");

        // Original snapshot should be unchanged
        assert_eq!(snap1.total_requests, 2);
        assert_eq!(snap1.successes, 2);
        assert_eq!(snap1.failures, 0);

        // New snapshot should reflect new data
        let snap2 = pm.snapshot_for("isolation").unwrap();
        assert_eq!(snap2.total_requests, 4);
        assert_eq!(snap2.successes, 3);
        assert_eq!(snap2.failures, 1);
    }

    #[test]
    fn provider_metrics_mixed_providers_concurrency() {
        use std::sync::Arc;
        use std::thread;

        let pm = Arc::new(ProviderMetrics::default());
        let mut handles = Vec::new();

        // Spawn threads for different providers
        let providers = vec!["email", "slack", "pagerduty", "webhook"];
        for provider in &providers {
            let pm_clone = Arc::clone(&pm);
            let provider_name = provider.to_string();
            let handle = thread::spawn(move || {
                for i in 0..500 {
                    pm_clone.record_success(&provider_name, (i * 100) as u64);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        let all = pm.snapshot();
        assert_eq!(all.len(), 4);

        for provider in &providers {
            let snap = all.get(*provider).unwrap();
            assert_eq!(snap.total_requests, 500);
            assert_eq!(snap.successes, 500);
            assert_eq!(snap.failures, 0);
        }
    }

    #[test]
    fn provider_metrics_zero_requests_success_rate() {
        let pm = ProviderMetrics::default();
        // Record nothing, then check snapshot
        pm.providers
            .write()
            .insert("empty".to_owned(), ProviderStats::default());

        let snap = pm.snapshot_for("empty").unwrap();
        assert_eq!(snap.total_requests, 0);
        // Success rate should be 0% when no requests (no data = 0%, not 100%)
        assert!((snap.success_rate - 0.0).abs() < f64::EPSILON);
        assert_eq!(snap.avg_latency_ms, 0.0);
    }

    #[test]
    fn provider_metrics_percentiles_empty_buffer() {
        let pm = ProviderMetrics::default();
        pm.providers
            .write()
            .insert("no_samples".to_owned(), ProviderStats::default());

        let snap = pm.snapshot_for("no_samples").unwrap();
        // With no samples, percentiles should be 0
        assert_eq!(snap.p50_latency_ms, 0.0);
        assert_eq!(snap.p95_latency_ms, 0.0);
        assert_eq!(snap.p99_latency_ms, 0.0);
    }

    #[test]
    fn provider_metrics_last_request_timestamp() {
        let pm = ProviderMetrics::default();

        // Before any requests, timestamp should be None
        pm.providers
            .write()
            .insert("timestamp_test".to_owned(), ProviderStats::default());
        let snap1 = pm.snapshot_for("timestamp_test").unwrap();
        assert!(snap1.last_request_at.is_none());

        // After recording, should have a timestamp
        pm.record_success("timestamp_test", 1_000);
        let snap2 = pm.snapshot_for("timestamp_test").unwrap();
        assert!(snap2.last_request_at.is_some());
        assert!(snap2.last_request_at.unwrap() > 0);
    }

    #[test]
    fn provider_metrics_latency_buffer_eviction() {
        let pm = ProviderMetrics::default();

        // Fill buffer exactly to max
        for i in 0..MAX_LATENCY_SAMPLES {
            pm.record_success("eviction", (i as u64) * 1_000);
        }

        let snap1 = pm.snapshot_for("eviction").unwrap();
        let p50_before = snap1.p50_latency_ms;

        // Add more samples to trigger eviction
        for i in 0..100 {
            pm.record_success("eviction", ((MAX_LATENCY_SAMPLES + i) as u64) * 1_000);
        }

        let snap2 = pm.snapshot_for("eviction").unwrap();
        // Percentiles should have changed due to eviction
        assert_ne!(p50_before, snap2.p50_latency_ms);
        // But total count should reflect all requests
        assert_eq!(snap2.total_requests, (MAX_LATENCY_SAMPLES + 100) as u64);
    }

    #[test]
    fn gateway_metrics_concurrent_increments() {
        use std::sync::Arc;
        use std::thread;

        let metrics = Arc::new(GatewayMetrics::default());
        let mut handles = Vec::new();

        // Spawn 20 threads, each incrementing different counters
        for _ in 0..20 {
            let m = Arc::clone(&metrics);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    m.increment_dispatched();
                    m.increment_executed();
                    m.increment_failed();
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("thread should complete");
        }

        let snap = metrics.snapshot();
        assert_eq!(snap.dispatched, 2_000);
        assert_eq!(snap.executed, 2_000);
        assert_eq!(snap.failed, 2_000);
    }

    #[test]
    fn gateway_metrics_snapshot_consistency() {
        let m = GatewayMetrics::default();

        // Increment various counters
        for _ in 0..10 {
            m.increment_dispatched();
        }
        for _ in 0..5 {
            m.increment_executed();
        }
        for _ in 0..3 {
            m.increment_failed();
        }

        // Take multiple snapshots and verify they're consistent at point-in-time
        let snap1 = m.snapshot();
        let snap2 = m.snapshot();

        assert_eq!(snap1, snap2);

        // Modify counters
        m.increment_dispatched();

        // Original snapshots should be unchanged
        let snap3 = m.snapshot();
        assert_ne!(snap1, snap3);
        assert_eq!(snap1.dispatched, 10);
        assert_eq!(snap3.dispatched, 11);
    }

    #[test]
    fn sanitize_error_truncation() {
        let long_msg = "x".repeat(600);
        let sanitized = super::sanitize_error_message(&long_msg);
        assert!(sanitized.len() <= MAX_ERROR_MESSAGE_LEN + 20); // +20 for "... (truncated)"
        assert!(sanitized.ends_with("... (truncated)"));
    }

    #[test]
    fn sanitize_error_url_credentials() {
        let msg = "Failed to connect to https://user:secret123@api.example.com/v1";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("user"));
        assert!(!sanitized.contains("secret123"));
        assert!(sanitized.contains("https://***:***@"));
    }

    #[test]
    fn sanitize_error_auth_headers() {
        let msg = "HTTP 401: Authorization: Bearer fake-test-token-000";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("fake-test-token-000"));
        assert!(sanitized.contains("Authorization: ***"));
    }

    #[test]
    fn sanitize_error_api_key() {
        let msg = "Invalid API-Key: AKIAIOSFODNN7EXAMPLE";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(sanitized.contains("API-Key: ***"));
    }

    #[test]
    fn sanitize_error_file_paths() {
        let msg = "File not found: /var/lib/acteon/secret.key";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("/var/lib/acteon/secret.key"));
        assert!(sanitized.contains("[PATH_REDACTED]"));
    }

    #[test]
    fn sanitize_error_windows_paths() {
        let msg = "Access denied: C:\\Program Files\\Acteon\\config.toml";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("C:\\Program Files\\Acteon\\config.toml"));
        assert!(sanitized.contains("[PATH_REDACTED]"));
    }

    #[test]
    fn sanitize_error_multiple_patterns() {
        let msg = "Failed to POST https://admin:pass@api.example.com/webhook with token=sk-123 at /etc/acteon/config.toml";
        let sanitized = super::sanitize_error_message(msg);
        assert!(!sanitized.contains("admin:pass"));
        assert!(!sanitized.contains("sk-123"));
        assert!(!sanitized.contains("/etc/acteon/config.toml"));
        assert!(sanitized.contains("https://***:***@"));
        assert!(sanitized.contains("token: ***"));
        assert!(sanitized.contains("[PATH_REDACTED]"));
    }

    #[test]
    fn sanitize_error_benign_message() {
        let msg = "Connection timeout after 30s";
        let sanitized = super::sanitize_error_message(msg);
        // Benign messages should pass through unchanged
        assert_eq!(sanitized, msg);
    }

    #[test]
    fn provider_metrics_max_providers_limit() {
        let pm = ProviderMetrics::default();

        // Fill up to the limit
        for i in 0..MAX_TRACKED_PROVIDERS {
            pm.record_success(&format!("provider-{i}"), 1_000);
        }

        let snap = pm.snapshot();
        assert_eq!(snap.len(), MAX_TRACKED_PROVIDERS);

        // Try to add one more provider (should be silently dropped)
        pm.record_success("provider-overflow", 1_000);
        let snap = pm.snapshot();
        assert_eq!(snap.len(), MAX_TRACKED_PROVIDERS);
        assert!(!snap.contains_key("provider-overflow"));

        // Existing providers should still work
        pm.record_success("provider-0", 2_000);
        let snap = pm.snapshot_for("provider-0").unwrap();
        assert_eq!(snap.total_requests, 2);
    }

    #[test]
    fn provider_metrics_max_providers_limit_failures() {
        let pm = ProviderMetrics::default();

        // Fill up to the limit with failures
        for i in 0..MAX_TRACKED_PROVIDERS {
            pm.record_failure(&format!("provider-{i}"), 1_000, "test error");
        }

        let snap = pm.snapshot();
        assert_eq!(snap.len(), MAX_TRACKED_PROVIDERS);

        // Try to add one more provider via failure (should be silently dropped)
        pm.record_failure("provider-overflow", 1_000, "test error");
        let snap = pm.snapshot();
        assert_eq!(snap.len(), MAX_TRACKED_PROVIDERS);
        assert!(!snap.contains_key("provider-overflow"));
    }
}
