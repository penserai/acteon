// Security review (2026-02-15):
//
// 1. No PII or tenant-specific data in metric labels. The only label is
//    `provider`, which is a registered provider name (not user input like
//    namespace or tenant). Aggregate counters carry no identifying info.
//
// 2. Cardinality explosion is bounded: `MAX_TRACKED_PROVIDERS = 1_000` in
//    metrics.rs prevents unbounded label growth from misconfigured clients.
//
// 3. Provider label values are escaped (backslash, double-quote, newline)
//    per the Prometheus text exposition format to prevent metric injection.
//
// 4. Error messages are NOT exposed in this endpoint (only in the JSON
//    /metrics endpoint). The underlying `sanitize_error_message()` in
//    metrics.rs redacts credentials, tokens, and file paths regardless.
//
// 5. The /metrics/prometheus endpoint is public (no auth required), which
//    is standard for Prometheus scraping. It only exposes aggregate counters
//    and per-provider stats -- no secrets or sensitive configuration.
//
// 6. Docker monitoring stack (Prometheus, Grafana) uses default passwords
//    suitable for local development only. Production deployments should
//    override GF_SECURITY_ADMIN_PASSWORD and enable Redis AUTH.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;

use acteon_gateway::{MetricsSnapshot, ProviderStatsSnapshot};

use super::AppState;

/// Prometheus text exposition format content type.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// `GET /metrics/prometheus` -- returns gateway metrics in Prometheus text exposition format.
///
/// Exports all gateway counters, embedding metrics, and per-provider execution
/// stats for scraping by Prometheus. Each metric uses the `acteon_` prefix.
///
/// **Naming convention note**: Provider latency gauges use `_ms` (milliseconds)
/// rather than base-unit seconds, and `success_rate` is a percentage (0-100)
/// rather than a ratio (0-1). This deviates from Prometheus naming best practices
/// but matches the internal representation and avoids lossy float conversions.
#[utoipa::path(
    get,
    path = "/metrics/prometheus",
    tag = "Health",
    summary = "Prometheus metrics",
    description = "Returns all gateway metrics in Prometheus text exposition format for scraping.",
    responses(
        (status = 200, description = "Prometheus text format metrics", content_type = "text/plain")
    )
)]
#[allow(clippy::unused_async)]
pub async fn prometheus_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let gw = state.gateway.read().await;
    let snap = gw.metrics().snapshot();

    let mut buf = render_snapshot(&snap);

    // -- Embedding metrics (optional) --
    if let Some(em) = state.embedding_metrics.as_ref() {
        let es = em.snapshot();
        write_counter(
            &mut buf,
            "acteon_embedding_topic_cache_hits_total",
            "Topic embeddings served from cache.",
            es.topic_cache_hits,
        );
        write_counter(
            &mut buf,
            "acteon_embedding_topic_cache_misses_total",
            "Topic embeddings requiring provider API call.",
            es.topic_cache_misses,
        );
        write_counter(
            &mut buf,
            "acteon_embedding_text_cache_hits_total",
            "Text embeddings served from cache.",
            es.text_cache_hits,
        );
        write_counter(
            &mut buf,
            "acteon_embedding_text_cache_misses_total",
            "Text embeddings requiring provider API call.",
            es.text_cache_misses,
        );
        write_counter(
            &mut buf,
            "acteon_embedding_errors_total",
            "Total embedding provider errors.",
            es.errors,
        );
        write_counter(
            &mut buf,
            "acteon_embedding_fail_open_total",
            "Times fail-open returned similarity 0.0 instead of an error.",
            es.fail_open_count,
        );
    }

    // -- Per-provider execution metrics --
    let provider_stats = gw.provider_metrics().snapshot();
    render_provider_metrics(&mut buf, &provider_stats);

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        buf,
    )
}

/// Render a `MetricsSnapshot` into Prometheus text exposition format.
///
/// Extracted from the handler so it can be unit tested without `AppState`
/// or an async runtime.
#[allow(clippy::too_many_lines)]
fn render_snapshot(snap: &MetricsSnapshot) -> String {
    let mut buf = String::with_capacity(4096);

    // -- Gateway dispatch counters --
    write_counter(
        &mut buf,
        "acteon_actions_dispatched_total",
        "Total number of actions dispatched to the gateway.",
        snap.dispatched,
    );
    write_counter(
        &mut buf,
        "acteon_actions_executed_total",
        "Actions successfully executed by a provider.",
        snap.executed,
    );
    write_counter(
        &mut buf,
        "acteon_actions_deduplicated_total",
        "Actions skipped as already processed (deduplication).",
        snap.deduplicated,
    );
    write_counter(
        &mut buf,
        "acteon_actions_suppressed_total",
        "Actions suppressed by a matching rule.",
        snap.suppressed,
    );
    write_counter(
        &mut buf,
        "acteon_actions_rerouted_total",
        "Actions rerouted to a different provider.",
        snap.rerouted,
    );
    write_counter(
        &mut buf,
        "acteon_actions_throttled_total",
        "Actions rejected due to rate limiting.",
        snap.throttled,
    );
    write_counter(
        &mut buf,
        "acteon_actions_failed_total",
        "Actions that failed after all retries.",
        snap.failed,
    );
    write_counter(
        &mut buf,
        "acteon_actions_pending_approval_total",
        "Actions sent to human approval workflow.",
        snap.pending_approval,
    );
    write_counter(
        &mut buf,
        "acteon_actions_scheduled_total",
        "Actions scheduled for delayed execution.",
        snap.scheduled,
    );

    // -- LLM guardrail counters --
    write_counter(
        &mut buf,
        "acteon_llm_guardrail_allowed_total",
        "Actions approved by the LLM guardrail.",
        snap.llm_guardrail_allowed,
    );
    write_counter(
        &mut buf,
        "acteon_llm_guardrail_denied_total",
        "Actions blocked by the LLM guardrail.",
        snap.llm_guardrail_denied,
    );
    write_counter(
        &mut buf,
        "acteon_llm_guardrail_errors_total",
        "LLM guardrail evaluation errors.",
        snap.llm_guardrail_errors,
    );

    // -- Chain (workflow) counters --
    write_counter(
        &mut buf,
        "acteon_chains_started_total",
        "Task chains initiated.",
        snap.chains_started,
    );
    write_counter(
        &mut buf,
        "acteon_chains_completed_total",
        "Task chains completed successfully.",
        snap.chains_completed,
    );
    write_counter(
        &mut buf,
        "acteon_chains_failed_total",
        "Task chains that failed.",
        snap.chains_failed,
    );
    write_counter(
        &mut buf,
        "acteon_chains_cancelled_total",
        "Task chains cancelled.",
        snap.chains_cancelled,
    );

    // -- Circuit breaker counters --
    write_counter(
        &mut buf,
        "acteon_circuit_open_total",
        "Actions rejected because the provider circuit breaker was open.",
        snap.circuit_open,
    );
    write_counter(
        &mut buf,
        "acteon_circuit_transitions_total",
        "Circuit breaker state transitions (any direction).",
        snap.circuit_transitions,
    );
    write_counter(
        &mut buf,
        "acteon_circuit_fallbacks_total",
        "Actions rerouted to a fallback provider due to an open circuit.",
        snap.circuit_fallbacks,
    );

    // -- Recurring action counters --
    write_counter(
        &mut buf,
        "acteon_recurring_dispatched_total",
        "Recurring actions successfully dispatched.",
        snap.recurring_dispatched,
    );
    write_counter(
        &mut buf,
        "acteon_recurring_errors_total",
        "Recurring action dispatch errors.",
        snap.recurring_errors,
    );
    write_counter(
        &mut buf,
        "acteon_recurring_skipped_total",
        "Recurring actions skipped (disabled, expired, etc.).",
        snap.recurring_skipped,
    );

    // -- Quota counters --
    write_counter(
        &mut buf,
        "acteon_quota_exceeded_total",
        "Actions blocked by tenant quota (HTTP 429).",
        snap.quota_exceeded,
    );
    write_counter(
        &mut buf,
        "acteon_quota_warned_total",
        "Actions that passed with a quota warning.",
        snap.quota_warned,
    );
    write_counter(
        &mut buf,
        "acteon_quota_degraded_total",
        "Actions degraded to a fallback provider due to quota.",
        snap.quota_degraded,
    );
    write_counter(
        &mut buf,
        "acteon_quota_notified_total",
        "Quota threshold notifications sent to tenant admin.",
        snap.quota_notified,
    );

    // -- Retention reaper counters --
    write_counter(
        &mut buf,
        "acteon_retention_deleted_state_total",
        "State entries deleted by the retention reaper.",
        snap.retention_deleted_state,
    );
    write_counter(
        &mut buf,
        "acteon_retention_skipped_compliance_total",
        "Entries skipped by retention reaper due to compliance hold.",
        snap.retention_skipped_compliance,
    );
    write_counter(
        &mut buf,
        "acteon_retention_errors_total",
        "Retention reaper processing errors.",
        snap.retention_errors,
    );
    write_counter(
        &mut buf,
        "acteon_wasm_invocations_total",
        "WASM plugin invocations.",
        snap.wasm_invocations,
    );
    write_counter(
        &mut buf,
        "acteon_wasm_errors_total",
        "WASM plugin invocation errors.",
        snap.wasm_errors,
    );

    buf
}

/// Render per-provider execution metrics into Prometheus text exposition format.
fn render_provider_metrics(
    buf: &mut String,
    provider_stats: &HashMap<String, ProviderStatsSnapshot>,
) {
    if provider_stats.is_empty() {
        return;
    }

    write_provider_counter_header(
        buf,
        "acteon_provider_requests_total",
        "Total requests to a provider.",
    );
    for (name, s) in provider_stats {
        write_labeled_value(
            buf,
            "acteon_provider_requests_total",
            name,
            s.total_requests,
        );
    }
    buf.push('\n');

    write_provider_counter_header(
        buf,
        "acteon_provider_successes_total",
        "Successful provider executions.",
    );
    for (name, s) in provider_stats {
        write_labeled_value(buf, "acteon_provider_successes_total", name, s.successes);
    }
    buf.push('\n');

    write_provider_counter_header(
        buf,
        "acteon_provider_failures_total",
        "Failed provider executions.",
    );
    for (name, s) in provider_stats {
        write_labeled_value(buf, "acteon_provider_failures_total", name, s.failures);
    }
    buf.push('\n');

    write_provider_gauge_header(
        buf,
        "acteon_provider_success_rate",
        "Provider success rate percentage (0-100).",
    );
    for (name, s) in provider_stats {
        write_labeled_float(buf, "acteon_provider_success_rate", name, s.success_rate);
    }
    buf.push('\n');

    write_provider_gauge_header(
        buf,
        "acteon_provider_avg_latency_ms",
        "Provider average latency in milliseconds.",
    );
    for (name, s) in provider_stats {
        write_labeled_float(
            buf,
            "acteon_provider_avg_latency_ms",
            name,
            s.avg_latency_ms,
        );
    }
    buf.push('\n');

    write_provider_gauge_header(
        buf,
        "acteon_provider_p50_latency_ms",
        "Provider 50th percentile (median) latency in milliseconds.",
    );
    for (name, s) in provider_stats {
        write_labeled_float(
            buf,
            "acteon_provider_p50_latency_ms",
            name,
            s.p50_latency_ms,
        );
    }
    buf.push('\n');

    write_provider_gauge_header(
        buf,
        "acteon_provider_p95_latency_ms",
        "Provider 95th percentile latency in milliseconds.",
    );
    for (name, s) in provider_stats {
        write_labeled_float(
            buf,
            "acteon_provider_p95_latency_ms",
            name,
            s.p95_latency_ms,
        );
    }
    buf.push('\n');

    write_provider_gauge_header(
        buf,
        "acteon_provider_p99_latency_ms",
        "Provider 99th percentile latency in milliseconds.",
    );
    for (name, s) in provider_stats {
        write_labeled_float(
            buf,
            "acteon_provider_p99_latency_ms",
            name,
            s.p99_latency_ms,
        );
    }
    buf.push('\n');
}

/// Write a single counter metric with HELP and TYPE annotations.
fn write_counter(buf: &mut String, name: &str, help: &str, value: u64) {
    use std::fmt::Write;
    let _ = writeln!(buf, "# HELP {name} {help}");
    let _ = writeln!(buf, "# TYPE {name} counter");
    let _ = writeln!(buf, "{name} {value}");
    buf.push('\n');
}

/// Write HELP and TYPE header for a counter with provider labels.
fn write_provider_counter_header(buf: &mut String, name: &str, help: &str) {
    use std::fmt::Write;
    let _ = writeln!(buf, "# HELP {name} {help}");
    let _ = writeln!(buf, "# TYPE {name} counter");
}

/// Write HELP and TYPE header for a gauge with provider labels.
fn write_provider_gauge_header(buf: &mut String, name: &str, help: &str) {
    use std::fmt::Write;
    let _ = writeln!(buf, "# HELP {name} {help}");
    let _ = writeln!(buf, "# TYPE {name} gauge");
}

/// Write a single metric line with a `provider` label and u64 value.
fn write_labeled_value(buf: &mut String, name: &str, provider: &str, value: u64) {
    use std::fmt::Write;
    let escaped = escape_label_value(provider);
    let _ = writeln!(buf, "{name}{{provider=\"{escaped}\"}} {value}");
}

/// Write a single metric line with a `provider` label and f64 value.
fn write_labeled_float(buf: &mut String, name: &str, provider: &str, value: f64) {
    use std::fmt::Write;
    let escaped = escape_label_value(provider);
    let _ = writeln!(buf, "{name}{{provider=\"{escaped}\"}} {value:.2}");
}

/// Escape a Prometheus label value per the text exposition format.
///
/// The spec requires that backslash, double-quote, and newline characters
/// inside label values are escaped. This prevents metric injection if a
/// provider name contains crafted characters.
fn escape_label_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper to build a zero-valued MetricsSnapshot --

    fn zero_snapshot() -> MetricsSnapshot {
        MetricsSnapshot {
            dispatched: 0,
            executed: 0,
            deduplicated: 0,
            suppressed: 0,
            rerouted: 0,
            throttled: 0,
            failed: 0,
            pending_approval: 0,
            llm_guardrail_allowed: 0,
            llm_guardrail_denied: 0,
            llm_guardrail_errors: 0,
            chains_started: 0,
            chains_completed: 0,
            chains_failed: 0,
            chains_cancelled: 0,
            circuit_open: 0,
            circuit_transitions: 0,
            circuit_fallbacks: 0,
            scheduled: 0,
            recurring_dispatched: 0,
            recurring_errors: 0,
            recurring_skipped: 0,
            quota_exceeded: 0,
            quota_warned: 0,
            quota_degraded: 0,
            quota_notified: 0,
            retention_deleted_state: 0,
            retention_skipped_compliance: 0,
            retention_errors: 0,
            wasm_invocations: 0,
            wasm_errors: 0,
        }
    }

    fn nonzero_snapshot() -> MetricsSnapshot {
        MetricsSnapshot {
            dispatched: 100,
            executed: 80,
            deduplicated: 5,
            suppressed: 3,
            rerouted: 2,
            throttled: 1,
            failed: 4,
            pending_approval: 1,
            llm_guardrail_allowed: 10,
            llm_guardrail_denied: 2,
            llm_guardrail_errors: 1,
            chains_started: 5,
            chains_completed: 3,
            chains_failed: 1,
            chains_cancelled: 1,
            circuit_open: 2,
            circuit_transitions: 3,
            circuit_fallbacks: 1,
            scheduled: 7,
            recurring_dispatched: 4,
            recurring_errors: 1,
            recurring_skipped: 2,
            quota_exceeded: 3,
            quota_warned: 2,
            quota_degraded: 1,
            quota_notified: 1,
            retention_deleted_state: 10,
            retention_skipped_compliance: 2,
            retention_errors: 1,
            wasm_invocations: 6,
            wasm_errors: 2,
        }
    }

    fn sample_provider_stats() -> ProviderStatsSnapshot {
        ProviderStatsSnapshot {
            total_requests: 50,
            successes: 45,
            failures: 5,
            success_rate: 90.0,
            avg_latency_ms: 12.34,
            p50_latency_ms: 10.0,
            p95_latency_ms: 25.5,
            p99_latency_ms: 42.0,
            last_request_at: Some(1_700_000_000_000),
            last_error: None,
        }
    }

    // -- Formatting helper tests --

    #[test]
    fn write_counter_format() {
        let mut buf = String::new();
        write_counter(&mut buf, "acteon_test_total", "A test counter.", 42);
        assert!(buf.contains("# HELP acteon_test_total A test counter."));
        assert!(buf.contains("# TYPE acteon_test_total counter"));
        assert!(buf.contains("acteon_test_total 42"));
    }

    #[test]
    fn write_counter_exact_line_format() {
        let mut buf = String::new();
        write_counter(&mut buf, "acteon_foo_total", "Help text.", 7);
        let lines: Vec<&str> = buf.lines().collect();
        assert_eq!(lines[0], "# HELP acteon_foo_total Help text.");
        assert_eq!(lines[1], "# TYPE acteon_foo_total counter");
        assert_eq!(lines[2], "acteon_foo_total 7");
    }

    #[test]
    fn write_counter_zero_value() {
        let mut buf = String::new();
        write_counter(&mut buf, "acteon_zero_total", "Zero counter.", 0);
        assert!(buf.contains("acteon_zero_total 0"));
    }

    #[test]
    fn write_counter_large_value() {
        let mut buf = String::new();
        write_counter(&mut buf, "acteon_large_total", "Large counter.", u64::MAX);
        assert!(buf.contains(&format!("acteon_large_total {}", u64::MAX)));
    }

    #[test]
    fn write_labeled_value_format() {
        let mut buf = String::new();
        write_labeled_value(&mut buf, "acteon_provider_requests_total", "email", 100);
        assert_eq!(
            buf.trim(),
            "acteon_provider_requests_total{provider=\"email\"} 100"
        );
    }

    #[test]
    fn write_labeled_float_format() {
        let mut buf = String::new();
        write_labeled_float(&mut buf, "acteon_provider_success_rate", "slack", 99.5);
        assert_eq!(
            buf.trim(),
            "acteon_provider_success_rate{provider=\"slack\"} 99.50"
        );
    }

    #[test]
    fn write_labeled_float_zero() {
        let mut buf = String::new();
        write_labeled_float(&mut buf, "acteon_provider_avg_latency_ms", "test", 0.0);
        assert_eq!(
            buf.trim(),
            "acteon_provider_avg_latency_ms{provider=\"test\"} 0.00"
        );
    }

    #[test]
    fn write_labeled_float_precision_two_decimals() {
        let mut buf = String::new();
        // Should truncate/round to 2 decimal places
        write_labeled_float(&mut buf, "m", "p", 1.0 / 3.0);
        assert!(buf.contains("0.33"));
        assert!(!buf.contains("0.333"));
    }

    #[test]
    fn write_labeled_float_whole_number() {
        let mut buf = String::new();
        write_labeled_float(&mut buf, "m", "p", 100.0);
        assert!(buf.contains("100.00"));
    }

    // -- Escape label value tests --

    #[test]
    fn escape_label_value_plain() {
        assert_eq!(escape_label_value("email"), "email");
    }

    #[test]
    fn escape_label_value_quotes() {
        assert_eq!(escape_label_value(r#"my"provider"#), r#"my\"provider"#);
    }

    #[test]
    fn escape_label_value_backslash() {
        assert_eq!(escape_label_value(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn escape_label_value_newline() {
        assert_eq!(escape_label_value("line\nbreak"), r"line\nbreak");
    }

    #[test]
    fn escape_label_value_all_special() {
        let input = "a\\b\"c\nd";
        let escaped = escape_label_value(input);
        assert_eq!(escaped, r#"a\\b\"c\nd"#);
    }

    #[test]
    fn escape_label_value_empty() {
        assert_eq!(escape_label_value(""), "");
    }

    #[test]
    fn escape_label_value_injection_attempt() {
        let malicious = "evil\"} fake_metric 999\n# ";
        let escaped = escape_label_value(malicious);
        assert!(!escaped.contains('\n'));
        let mut buf = String::new();
        write_labeled_value(&mut buf, "acteon_provider_requests_total", malicious, 1);
        let lines: Vec<&str> = buf.trim().lines().collect();
        assert_eq!(lines.len(), 1, "injection should not create extra lines");
    }

    // -- Provider header tests --

    #[test]
    fn write_provider_counter_header_format() {
        let mut buf = String::new();
        write_provider_counter_header(&mut buf, "acteon_provider_requests_total", "Help.");
        assert!(buf.contains("# HELP acteon_provider_requests_total Help."));
        assert!(buf.contains("# TYPE acteon_provider_requests_total counter"));
        // Should NOT contain "gauge"
        assert!(!buf.contains("gauge"));
    }

    #[test]
    fn write_provider_gauge_header_format() {
        let mut buf = String::new();
        write_provider_gauge_header(&mut buf, "acteon_provider_success_rate", "Rate.");
        assert!(buf.contains("# HELP acteon_provider_success_rate Rate."));
        assert!(buf.contains("# TYPE acteon_provider_success_rate gauge"));
        // Should NOT contain "counter"
        assert!(!buf.contains("counter"));
    }

    // -- render_snapshot tests --

    /// All 31 gateway counter metric names that must appear in the output.
    const EXPECTED_COUNTER_METRICS: &[&str] = &[
        "acteon_actions_dispatched_total",
        "acteon_actions_executed_total",
        "acteon_actions_deduplicated_total",
        "acteon_actions_suppressed_total",
        "acteon_actions_rerouted_total",
        "acteon_actions_throttled_total",
        "acteon_actions_failed_total",
        "acteon_actions_pending_approval_total",
        "acteon_actions_scheduled_total",
        "acteon_llm_guardrail_allowed_total",
        "acteon_llm_guardrail_denied_total",
        "acteon_llm_guardrail_errors_total",
        "acteon_chains_started_total",
        "acteon_chains_completed_total",
        "acteon_chains_failed_total",
        "acteon_chains_cancelled_total",
        "acteon_circuit_open_total",
        "acteon_circuit_transitions_total",
        "acteon_circuit_fallbacks_total",
        "acteon_recurring_dispatched_total",
        "acteon_recurring_errors_total",
        "acteon_recurring_skipped_total",
        "acteon_quota_exceeded_total",
        "acteon_quota_warned_total",
        "acteon_quota_degraded_total",
        "acteon_quota_notified_total",
        "acteon_retention_deleted_state_total",
        "acteon_retention_skipped_compliance_total",
        "acteon_retention_errors_total",
        "acteon_wasm_invocations_total",
        "acteon_wasm_errors_total",
    ];

    #[test]
    fn render_snapshot_contains_all_31_counter_metrics() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        for metric in EXPECTED_COUNTER_METRICS {
            assert!(output.contains(metric), "Missing metric: {metric}");
        }
    }

    #[test]
    fn render_snapshot_all_counters_have_help_and_type() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        for metric in EXPECTED_COUNTER_METRICS {
            let help_line = format!("# HELP {metric} ");
            let type_line = format!("# TYPE {metric} counter");
            assert!(
                output.contains(&help_line),
                "Missing HELP annotation for {metric}"
            );
            assert!(
                output.contains(&type_line),
                "Missing TYPE annotation for {metric}"
            );
        }
    }

    #[test]
    fn render_snapshot_zero_values() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        for metric in EXPECTED_COUNTER_METRICS {
            let value_line = format!("{metric} 0");
            assert!(
                output.contains(&value_line),
                "Expected '{value_line}' in output for zero snapshot"
            );
        }
    }

    #[test]
    fn render_snapshot_nonzero_values() {
        let snap = nonzero_snapshot();
        let output = render_snapshot(&snap);

        assert!(output.contains("acteon_actions_dispatched_total 100"));
        assert!(output.contains("acteon_actions_executed_total 80"));
        assert!(output.contains("acteon_actions_deduplicated_total 5"));
        assert!(output.contains("acteon_actions_suppressed_total 3"));
        assert!(output.contains("acteon_actions_rerouted_total 2"));
        assert!(output.contains("acteon_actions_throttled_total 1"));
        assert!(output.contains("acteon_actions_failed_total 4"));
        assert!(output.contains("acteon_actions_pending_approval_total 1"));
        assert!(output.contains("acteon_actions_scheduled_total 7"));
        assert!(output.contains("acteon_llm_guardrail_allowed_total 10"));
        assert!(output.contains("acteon_llm_guardrail_denied_total 2"));
        assert!(output.contains("acteon_llm_guardrail_errors_total 1"));
        assert!(output.contains("acteon_chains_started_total 5"));
        assert!(output.contains("acteon_chains_completed_total 3"));
        assert!(output.contains("acteon_chains_failed_total 1"));
        assert!(output.contains("acteon_chains_cancelled_total 1"));
        assert!(output.contains("acteon_circuit_open_total 2"));
        assert!(output.contains("acteon_circuit_transitions_total 3"));
        assert!(output.contains("acteon_circuit_fallbacks_total 1"));
        assert!(output.contains("acteon_recurring_dispatched_total 4"));
        assert!(output.contains("acteon_recurring_errors_total 1"));
        assert!(output.contains("acteon_recurring_skipped_total 2"));
        assert!(output.contains("acteon_quota_exceeded_total 3"));
        assert!(output.contains("acteon_quota_warned_total 2"));
        assert!(output.contains("acteon_quota_degraded_total 1"));
        assert!(output.contains("acteon_quota_notified_total 1"));
        assert!(output.contains("acteon_retention_deleted_state_total 10"));
        assert!(output.contains("acteon_retention_skipped_compliance_total 2"));
        assert!(output.contains("acteon_retention_errors_total 1"));
        assert!(output.contains("acteon_wasm_invocations_total 6"));
        assert!(output.contains("acteon_wasm_errors_total 2"));
    }

    #[test]
    fn render_snapshot_help_text_not_empty() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        for line in output.lines() {
            if line.starts_with("# HELP ") {
                // Format: "# HELP metric_name Some help text."
                let after_help = line.strip_prefix("# HELP ").unwrap();
                let parts: Vec<&str> = after_help.splitn(2, ' ').collect();
                assert!(
                    parts.len() == 2 && !parts[1].is_empty(),
                    "HELP line should have non-empty description: {line}"
                );
            }
        }
    }

    #[test]
    fn render_snapshot_no_duplicate_type_declarations() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        let type_lines: Vec<&str> = output
            .lines()
            .filter(|l| l.starts_with("# TYPE "))
            .collect();
        let unique_count = type_lines
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(
            type_lines.len(),
            unique_count,
            "Found duplicate TYPE declarations"
        );
    }

    #[test]
    fn render_snapshot_type_before_value() {
        let snap = zero_snapshot();
        let output = render_snapshot(&snap);

        for metric in EXPECTED_COUNTER_METRICS {
            let type_pos = output
                .find(&format!("# TYPE {metric} counter"))
                .unwrap_or_else(|| panic!("Missing TYPE for {metric}"));
            let value_line = format!("\n{metric} ");
            let value_pos = output
                .find(&value_line)
                .unwrap_or_else(|| panic!("Missing value line for {metric}"));
            assert!(
                type_pos < value_pos,
                "TYPE must come before value for {metric}"
            );
        }
    }

    #[test]
    fn render_snapshot_all_lines_valid_prometheus_format() {
        let snap = nonzero_snapshot();
        let output = render_snapshot(&snap);

        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            let is_comment = line.starts_with('#');
            let is_metric = !is_comment
                && line
                    .split_whitespace()
                    .last()
                    .map(|v| v.parse::<f64>().is_ok())
                    .unwrap_or(false);
            assert!(
                is_comment || is_metric,
                "Line is neither comment nor valid metric: {line}"
            );
        }
    }

    // -- render_provider_metrics tests --

    #[test]
    fn render_provider_metrics_empty_map() {
        let mut buf = String::new();
        let empty: HashMap<String, ProviderStatsSnapshot> = HashMap::new();
        render_provider_metrics(&mut buf, &empty);
        assert!(
            buf.is_empty(),
            "Empty provider map should produce no output"
        );
    }

    /// The 8 per-provider metric families.
    const PROVIDER_METRIC_FAMILIES: &[(&str, &str)] = &[
        ("acteon_provider_requests_total", "counter"),
        ("acteon_provider_successes_total", "counter"),
        ("acteon_provider_failures_total", "counter"),
        ("acteon_provider_success_rate", "gauge"),
        ("acteon_provider_avg_latency_ms", "gauge"),
        ("acteon_provider_p50_latency_ms", "gauge"),
        ("acteon_provider_p95_latency_ms", "gauge"),
        ("acteon_provider_p99_latency_ms", "gauge"),
    ];

    #[test]
    fn render_provider_metrics_single_provider() {
        let mut buf = String::new();
        let mut map = HashMap::new();
        map.insert("email".to_string(), sample_provider_stats());
        render_provider_metrics(&mut buf, &map);

        for (name, typ) in PROVIDER_METRIC_FAMILIES {
            assert!(
                buf.contains(&format!("# HELP {name} ")),
                "Missing HELP for {name}"
            );
            assert!(
                buf.contains(&format!("# TYPE {name} {typ}")),
                "Missing TYPE for {name}"
            );
            assert!(
                buf.contains(&format!("{name}{{provider=\"email\"}}")),
                "Missing labeled line for {name}"
            );
        }
    }

    #[test]
    fn render_provider_metrics_values_correct() {
        let mut buf = String::new();
        let mut map = HashMap::new();
        map.insert("slack".to_string(), sample_provider_stats());
        render_provider_metrics(&mut buf, &map);

        assert!(buf.contains("acteon_provider_requests_total{provider=\"slack\"} 50"));
        assert!(buf.contains("acteon_provider_successes_total{provider=\"slack\"} 45"));
        assert!(buf.contains("acteon_provider_failures_total{provider=\"slack\"} 5"));
        assert!(buf.contains("acteon_provider_success_rate{provider=\"slack\"} 90.00"));
        assert!(buf.contains("acteon_provider_avg_latency_ms{provider=\"slack\"} 12.34"));
        assert!(buf.contains("acteon_provider_p50_latency_ms{provider=\"slack\"} 10.00"));
        assert!(buf.contains("acteon_provider_p95_latency_ms{provider=\"slack\"} 25.50"));
        assert!(buf.contains("acteon_provider_p99_latency_ms{provider=\"slack\"} 42.00"));
    }

    #[test]
    fn render_provider_metrics_special_chars_escaped() {
        let mut buf = String::new();
        let mut map = HashMap::new();
        map.insert("my\"evil\nprovider".to_string(), sample_provider_stats());
        render_provider_metrics(&mut buf, &map);

        // The escaped label should appear in the output
        assert!(buf.contains(r#"provider="my\"evil\nprovider""#));
        // No raw newlines inside metric lines
        for line in buf.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            assert!(
                !line.contains("evil\nprovider"),
                "Raw newline should be escaped"
            );
        }
    }

    #[test]
    fn render_provider_metrics_counter_type_correct() {
        let mut buf = String::new();
        let mut map = HashMap::new();
        map.insert("test".to_string(), sample_provider_stats());
        render_provider_metrics(&mut buf, &map);

        // Counter metrics should have TYPE counter
        assert!(buf.contains("# TYPE acteon_provider_requests_total counter"));
        assert!(buf.contains("# TYPE acteon_provider_successes_total counter"));
        assert!(buf.contains("# TYPE acteon_provider_failures_total counter"));

        // Gauge metrics should have TYPE gauge
        assert!(buf.contains("# TYPE acteon_provider_success_rate gauge"));
        assert!(buf.contains("# TYPE acteon_provider_avg_latency_ms gauge"));
        assert!(buf.contains("# TYPE acteon_provider_p50_latency_ms gauge"));
        assert!(buf.contains("# TYPE acteon_provider_p95_latency_ms gauge"));
        assert!(buf.contains("# TYPE acteon_provider_p99_latency_ms gauge"));
    }

    #[test]
    fn render_provider_metrics_zero_stats() {
        let mut buf = String::new();
        let mut map = HashMap::new();
        map.insert(
            "empty".to_string(),
            ProviderStatsSnapshot {
                total_requests: 0,
                successes: 0,
                failures: 0,
                success_rate: 0.0,
                avg_latency_ms: 0.0,
                p50_latency_ms: 0.0,
                p95_latency_ms: 0.0,
                p99_latency_ms: 0.0,
                last_request_at: None,
                last_error: None,
            },
        );
        render_provider_metrics(&mut buf, &map);

        assert!(buf.contains("acteon_provider_requests_total{provider=\"empty\"} 0"));
        assert!(buf.contains("acteon_provider_success_rate{provider=\"empty\"} 0.00"));
    }

    // -- Full render (snapshot + provider) integration test --

    #[test]
    fn full_render_snapshot_plus_providers_all_37_metrics() {
        let snap = nonzero_snapshot();
        let mut output = render_snapshot(&snap);

        let mut providers = HashMap::new();
        providers.insert("email".to_string(), sample_provider_stats());
        render_provider_metrics(&mut output, &providers);

        // 31 counter metrics from the snapshot
        for metric in EXPECTED_COUNTER_METRICS {
            assert!(output.contains(metric), "Missing snapshot metric: {metric}");
        }

        // 8 provider metric families
        for (name, _) in PROVIDER_METRIC_FAMILIES {
            assert!(output.contains(name), "Missing provider metric: {name}");
        }

        // Total: 31 + 8 = 39 unique metric families
        let type_lines: Vec<&str> = output
            .lines()
            .filter(|l| l.starts_with("# TYPE "))
            .collect();
        assert_eq!(type_lines.len(), 39, "Expected 39 TYPE declarations");
    }

    #[test]
    fn full_render_no_providers_has_31_type_lines() {
        let snap = zero_snapshot();
        let mut output = render_snapshot(&snap);
        let empty: HashMap<String, ProviderStatsSnapshot> = HashMap::new();
        render_provider_metrics(&mut output, &empty);

        let type_lines: Vec<&str> = output
            .lines()
            .filter(|l| l.starts_with("# TYPE "))
            .collect();
        assert_eq!(
            type_lines.len(),
            31,
            "Expected 31 TYPE declarations without providers"
        );
    }
}
