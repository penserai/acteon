//! Demonstration of monitoring metrics collection in Acteon.
//!
//! This simulation shows how gateway and per-provider metrics accumulate
//! during varied traffic patterns, and ends with a simulated Prometheus
//! text exposition output that represents what Grafana would scrape.
//!
//! Run with: `cargo run -p acteon-simulation --example monitoring_simulation`

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use acteon_core::Action;
use acteon_executor::ExecutorConfig;
use acteon_gateway::{
    CircuitBreakerConfig, GatewayBuilder, MetricsSnapshot, ProviderStatsSnapshot,
};
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::provider::{FailureMode, RecordingProvider};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

/// High circuit breaker threshold that effectively disables tripping.
fn high_threshold_cb() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        failure_threshold: 1000,
        success_threshold: 1,
        recovery_timeout: Duration::from_secs(3600),
        fallback_provider: None,
    }
}

/// Build a gateway with the given providers and a circuit breaker config.
fn build_gateway(
    providers: Vec<Arc<RecordingProvider>>,
    cb_config: CircuitBreakerConfig,
    rules_yaml: Option<&str>,
) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .executor_config(ExecutorConfig {
            max_retries: 0,
            ..ExecutorConfig::default()
        })
        .circuit_breaker(cb_config);

    for p in providers {
        builder = builder.provider(p as Arc<dyn acteon_provider::DynProvider>);
    }

    if let Some(yaml) = rules_yaml {
        let frontend = YamlFrontend;
        let rules = RuleFrontend::parse(&frontend, yaml).expect("valid YAML rules");
        builder = builder.rules(rules);
    }

    builder.build().expect("gateway should build")
}

fn make_action(ns: &str, tenant: &str, provider: &str, action_type: &str) -> Action {
    Action::new(
        ns,
        tenant,
        provider,
        action_type,
        serde_json::json!({
            "message": format!("Monitoring simulation: {action_type}"),
        }),
    )
}

fn make_action_with_payload(
    ns: &str,
    tenant: &str,
    provider: &str,
    action_type: &str,
    payload: serde_json::Value,
) -> Action {
    Action::new(ns, tenant, provider, action_type, payload)
}

/// Print a metrics snapshot in a readable table format.
fn print_metrics_snapshot(label: &str, snap: &MetricsSnapshot) {
    println!("  [{label}]");
    println!(
        "    dispatched={}, executed={}, suppressed={}, rerouted={}, \
         failed={}, throttled={}, deduplicated={}, circuit_open={}",
        snap.dispatched,
        snap.executed,
        snap.suppressed,
        snap.rerouted,
        snap.failed,
        snap.throttled,
        snap.deduplicated,
        snap.circuit_open
    );
}

/// Write gateway-level counter metrics in Prometheus text format.
fn render_gateway_counters(buf: &mut String, snap: &MetricsSnapshot) {
    let counters = [
        (
            "acteon_actions_dispatched_total",
            "Total number of actions dispatched to the gateway.",
            snap.dispatched,
        ),
        (
            "acteon_actions_executed_total",
            "Actions successfully executed by a provider.",
            snap.executed,
        ),
        (
            "acteon_actions_deduplicated_total",
            "Actions skipped as already processed (deduplication).",
            snap.deduplicated,
        ),
        (
            "acteon_actions_suppressed_total",
            "Actions suppressed by a matching rule.",
            snap.suppressed,
        ),
        (
            "acteon_actions_rerouted_total",
            "Actions rerouted to a different provider.",
            snap.rerouted,
        ),
        (
            "acteon_actions_throttled_total",
            "Actions rejected due to rate limiting.",
            snap.throttled,
        ),
        (
            "acteon_actions_failed_total",
            "Actions that failed after all retries.",
            snap.failed,
        ),
        (
            "acteon_circuit_open_total",
            "Actions rejected because the provider circuit breaker was open.",
            snap.circuit_open,
        ),
        (
            "acteon_circuit_transitions_total",
            "Circuit breaker state transitions (any direction).",
            snap.circuit_transitions,
        ),
        (
            "acteon_circuit_fallbacks_total",
            "Actions rerouted to a fallback provider due to an open circuit.",
            snap.circuit_fallbacks,
        ),
    ];

    for (name, help, value) in &counters {
        let _ = writeln!(buf, "# HELP {name} {help}");
        let _ = writeln!(buf, "# TYPE {name} counter");
        let _ = writeln!(buf, "{name} {value}");
        buf.push('\n');
    }
}

/// Write per-provider metrics in Prometheus text format.
fn render_provider_metrics(
    buf: &mut String,
    provider_stats: &HashMap<String, ProviderStatsSnapshot>,
) {
    if provider_stats.is_empty() {
        return;
    }

    // Helper: write a counter metric family with u64 values.
    let write_counter_family =
        |buf: &mut String, name: &str, help: &str, extract: fn(&ProviderStatsSnapshot) -> u64| {
            let _ = writeln!(buf, "# HELP {name} {help}");
            let _ = writeln!(buf, "# TYPE {name} counter");
            for (pname, s) in provider_stats {
                let _ = writeln!(buf, "{name}{{provider=\"{pname}\"}} {}", extract(s));
            }
            buf.push('\n');
        };

    // Helper: write a gauge metric family with f64 values.
    let write_gauge_family =
        |buf: &mut String, name: &str, help: &str, extract: fn(&ProviderStatsSnapshot) -> f64| {
            let _ = writeln!(buf, "# HELP {name} {help}");
            let _ = writeln!(buf, "# TYPE {name} gauge");
            for (pname, s) in provider_stats {
                let _ = writeln!(buf, "{name}{{provider=\"{pname}\"}} {:.2}", extract(s));
            }
            buf.push('\n');
        };

    write_counter_family(
        buf,
        "acteon_provider_requests_total",
        "Total requests to a provider.",
        |s| s.total_requests,
    );
    write_counter_family(
        buf,
        "acteon_provider_successes_total",
        "Successful provider executions.",
        |s| s.successes,
    );
    write_counter_family(
        buf,
        "acteon_provider_failures_total",
        "Failed provider executions.",
        |s| s.failures,
    );
    write_gauge_family(
        buf,
        "acteon_provider_success_rate",
        "Provider success rate percentage (0-100).",
        |s| s.success_rate,
    );
    write_gauge_family(
        buf,
        "acteon_provider_avg_latency_ms",
        "Provider average latency in milliseconds.",
        |s| s.avg_latency_ms,
    );
    write_gauge_family(
        buf,
        "acteon_provider_p99_latency_ms",
        "Provider 99th percentile latency in milliseconds.",
        |s| s.p99_latency_ms,
    );
}

/// Generate simulated Prometheus text exposition output from gateway state.
fn render_prometheus_output(
    snap: &MetricsSnapshot,
    provider_stats: &HashMap<String, ProviderStatsSnapshot>,
) -> String {
    let mut buf = String::with_capacity(4096);
    render_gateway_counters(&mut buf, snap);
    render_provider_metrics(&mut buf, provider_stats);
    buf
}

const SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-internal
    priority: 1
    condition:
      field: action.action_type
      eq: "internal_debug"
    action:
      type: suppress
"#;

const REROUTE_RULE: &str = r#"
rules:
  - name: reroute-critical
    priority: 2
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: webhook
"#;

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-alerts
    priority: 3
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

/// Scenario 1: Varied traffic patterns showing how counters accumulate.
async fn scenario_varied_traffic() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: VARIED TRAFFIC PATTERNS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    println!("  Dispatch a mix of successful, suppressed, rerouted, and");
    println!("  deduplicated actions. Watch counters accumulate over waves.\n");

    let combined_rules = format!("{SUPPRESSION_RULE}\n{REROUTE_RULE}\n{DEDUP_RULE}");

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack"));
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let gateway = build_gateway(
        vec![Arc::clone(&email), Arc::clone(&slack), Arc::clone(&webhook)],
        high_threshold_cb(),
        Some(&combined_rules),
    );

    // Wave 1: Normal traffic
    println!("  Wave 1: Normal traffic (email + slack)");
    for _ in 0..10 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "email", "send_email"),
                None,
            )
            .await?;
        gateway
            .dispatch(
                make_action("monitoring", "acme", "slack", "send_message"),
                None,
            )
            .await?;
    }
    let snap1 = gateway.metrics().snapshot();
    print_metrics_snapshot("After Wave 1", &snap1);
    assert_eq!(snap1.dispatched, 20);
    assert_eq!(snap1.executed, 20);

    // Wave 2: Suppressed traffic
    println!("\n  Wave 2: Internal debug traffic (suppressed)");
    for _ in 0..5 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "email", "internal_debug"),
                None,
            )
            .await?;
    }
    let snap2 = gateway.metrics().snapshot();
    print_metrics_snapshot("After Wave 2", &snap2);
    assert_eq!(snap2.dispatched, 25);
    assert_eq!(snap2.suppressed, 5);

    // Wave 3: Rerouted traffic (critical alerts reroute from slack to webhook)
    println!("\n  Wave 3: Critical alerts (rerouted slack -> webhook)");
    for _ in 0..8 {
        gateway
            .dispatch(
                make_action_with_payload(
                    "monitoring",
                    "acme",
                    "slack",
                    "alert",
                    serde_json::json!({ "severity": "critical", "source": "database" }),
                ),
                None,
            )
            .await?;
    }
    let snap3 = gateway.metrics().snapshot();
    print_metrics_snapshot("After Wave 3", &snap3);
    assert_eq!(snap3.rerouted, 8);

    // Wave 4: Deduplicated traffic
    println!("\n  Wave 4: Duplicate alerts (deduplicated)");
    let dedup_action =
        make_action("monitoring", "acme", "email", "alert").with_dedup_key("user-alert-001");
    gateway.dispatch(dedup_action, None).await?;
    for _ in 0..4 {
        let dup =
            make_action("monitoring", "acme", "email", "alert").with_dedup_key("user-alert-001");
        gateway.dispatch(dup, None).await?;
    }
    let snap4 = gateway.metrics().snapshot();
    print_metrics_snapshot("After Wave 4", &snap4);
    assert_eq!(snap4.deduplicated, 4);

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");
    Ok(())
}

/// Scenario 2: Per-provider metrics with different provider characteristics.
async fn scenario_per_provider_metrics() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: PER-PROVIDER METRICS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    println!("  Three providers with different characteristics:");
    println!("    - email: healthy, fast");
    println!("    - slack: healthy, 50ms delay");
    println!("    - webhook: 30% failure rate\n");

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack").with_delay(Duration::from_millis(50)));
    let webhook =
        Arc::new(RecordingProvider::new("webhook").with_failure_mode(FailureMode::EveryN(3)));

    let gateway = build_gateway(
        vec![Arc::clone(&email), Arc::clone(&slack), Arc::clone(&webhook)],
        high_threshold_cb(),
        None,
    );

    for _ in 0..30 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "email", "send_email"),
                None,
            )
            .await?;
        gateway
            .dispatch(
                make_action("monitoring", "acme", "slack", "send_message"),
                None,
            )
            .await?;
        let _ = gateway
            .dispatch(
                make_action("monitoring", "acme", "webhook", "post_hook"),
                None,
            )
            .await;
    }

    let provider_snap = gateway.provider_metrics().snapshot();
    println!("  Per-Provider Health Summary:");
    println!("  ┌──────────┬──────────┬───────────┬──────────┬────────────┬──────────────┐");
    println!("  │ Provider │ Requests │ Successes │ Failures │ Success %  │ Avg Lat (ms) │");
    println!("  ├──────────┼──────────┼───────────┼──────────┼────────────┼──────────────┤");
    for name in ["email", "slack", "webhook"] {
        let stats = provider_snap.get(name).unwrap();
        println!(
            "  │ {:8} │ {:8} │ {:9} │ {:8} │ {:9.1}% │ {:12.2} │",
            name,
            stats.total_requests,
            stats.successes,
            stats.failures,
            stats.success_rate,
            stats.avg_latency_ms,
        );
    }
    println!("  └──────────┴──────────┴───────────┴──────────┴────────────┴──────────────┘");

    let email_stats = provider_snap.get("email").unwrap();
    assert_eq!(email_stats.total_requests, 30);
    assert_eq!(email_stats.successes, 30);
    assert!((email_stats.success_rate - 100.0).abs() < f64::EPSILON);

    let slack_stats = provider_snap.get("slack").unwrap();
    assert_eq!(slack_stats.total_requests, 30);
    assert!(slack_stats.avg_latency_ms >= 50.0);

    let webhook_stats = provider_snap.get("webhook").unwrap();
    assert_eq!(webhook_stats.total_requests, 30);
    assert!(webhook_stats.failures >= 9);

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");
    Ok(())
}

/// Scenario 3: Circuit breaker metrics under provider failure.
async fn scenario_circuit_breaker_metrics() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: CIRCUIT BREAKER METRICS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    println!("  A failing provider triggers the circuit breaker.");
    println!("  Watch circuit_open and circuit_transitions counters.\n");

    let failing =
        Arc::new(RecordingProvider::new("failing").with_failure_mode(FailureMode::Always));
    let healthy = Arc::new(RecordingProvider::new("healthy"));

    let gateway = build_gateway(
        vec![Arc::clone(&failing), Arc::clone(&healthy)],
        CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        },
        None,
    );

    println!("  Sending requests to failing provider:");
    for i in 1..=8 {
        let outcome = gateway
            .dispatch(
                make_action("monitoring", "acme", "failing", "send_alert"),
                None,
            )
            .await;
        let snap = gateway.metrics().snapshot();
        let provider_failures = gateway
            .provider_metrics()
            .snapshot_for("failing")
            .map_or(0, |s| s.failures);
        let outcome_str = outcome
            .as_ref()
            .map_or_else(|e| format!("Err({e})"), |o| format!("{o:?}"));
        println!(
            "    Request {i}: outcome={outcome_str}, failed={provider_failures}, \
             circuit_open={}, transitions={}",
            snap.circuit_open, snap.circuit_transitions,
        );
    }

    for _ in 0..5 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "healthy", "send_alert"),
                None,
            )
            .await?;
    }

    let final_snap = gateway.metrics().snapshot();
    let healthy_stats = gateway.provider_metrics().snapshot_for("healthy").unwrap();

    println!("\n  Final metrics:");
    println!("    circuit_open = {}", final_snap.circuit_open);
    println!(
        "    circuit_transitions = {}",
        final_snap.circuit_transitions
    );
    println!(
        "    healthy provider: {} requests, {:.1}% success",
        healthy_stats.total_requests, healthy_stats.success_rate
    );

    assert_eq!(final_snap.circuit_open, 5);
    assert!(final_snap.circuit_transitions >= 1);
    assert_eq!(healthy_stats.total_requests, 5);
    assert_eq!(healthy_stats.successes, 5);

    gateway.shutdown().await;
    println!("\n  [Scenario 3 passed]\n");
    Ok(())
}

/// Scenario 4: Periodic snapshots simulating Prometheus scrape intervals.
async fn scenario_periodic_snapshots() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: PERIODIC METRICS SNAPSHOTS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    println!("  Simulating traffic in timed bursts and snapshotting metrics");
    println!("  after each burst, like a Prometheus scrape interval.\n");

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack"));
    let webhook =
        Arc::new(RecordingProvider::new("webhook").with_failure_mode(FailureMode::EveryN(5)));

    let gateway = build_gateway(
        vec![Arc::clone(&email), Arc::clone(&slack), Arc::clone(&webhook)],
        high_threshold_cb(),
        None,
    );

    let mut previous_dispatched = 0u64;
    for burst in 1..=4u64 {
        let burst_size = burst * 5;
        for _ in 0..burst_size {
            gateway
                .dispatch(
                    make_action("monitoring", "acme", "email", "send_email"),
                    None,
                )
                .await?;
            let _ = gateway
                .dispatch(
                    make_action("monitoring", "acme", "webhook", "post_hook"),
                    None,
                )
                .await;
        }

        let snap = gateway.metrics().snapshot();
        let delta = snap.dispatched - previous_dispatched;
        println!(
            "  Scrape {burst}: dispatched={} (+{delta}), executed={}, failed={}",
            snap.dispatched, snap.executed, snap.failed,
        );
        previous_dispatched = snap.dispatched;
    }

    let final_snap = gateway.metrics().snapshot();
    assert_eq!(final_snap.dispatched, 100);
    assert!(final_snap.failed > 0);

    gateway.shutdown().await;
    println!("\n  [Scenario 4 passed]\n");
    Ok(())
}

/// Scenario 5: Full Prometheus text exposition output.
async fn scenario_prometheus_output() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: SIMULATED PROMETHEUS EXPOSITION OUTPUT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    println!("  Building up realistic traffic across multiple providers,");
    println!("  then rendering the full Prometheus text output.\n");

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack").with_delay(Duration::from_millis(25)));
    let webhook =
        Arc::new(RecordingProvider::new("webhook").with_failure_mode(FailureMode::EveryN(4)));

    let gateway = build_gateway(
        vec![Arc::clone(&email), Arc::clone(&slack), Arc::clone(&webhook)],
        high_threshold_cb(),
        Some(SUPPRESSION_RULE),
    );

    for _ in 0..20 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "email", "send_email"),
                None,
            )
            .await?;
        gateway
            .dispatch(
                make_action("monitoring", "acme", "slack", "send_message"),
                None,
            )
            .await?;
        let _ = gateway
            .dispatch(
                make_action("monitoring", "acme", "webhook", "post_hook"),
                None,
            )
            .await;
    }

    for _ in 0..5 {
        gateway
            .dispatch(
                make_action("monitoring", "acme", "email", "internal_debug"),
                None,
            )
            .await?;
    }

    let metrics_snap = gateway.metrics().snapshot();
    let provider_snap = gateway.provider_metrics().snapshot();
    let prom_output = render_prometheus_output(&metrics_snap, &provider_snap);

    println!("  --- BEGIN Prometheus text exposition ---");
    for line in prom_output.lines() {
        println!("  {line}");
    }
    println!("  --- END Prometheus text exposition ---\n");

    assert!(prom_output.contains("acteon_actions_dispatched_total"));
    assert!(prom_output.contains("acteon_actions_executed_total"));
    assert!(prom_output.contains("acteon_actions_suppressed_total"));
    assert!(prom_output.contains("acteon_circuit_open_total"));
    assert!(prom_output.contains("acteon_provider_requests_total{provider=\"email\"}"));
    assert!(prom_output.contains("acteon_provider_success_rate{provider=\"slack\"}"));
    assert!(prom_output.contains("acteon_provider_p99_latency_ms"));
    assert!(prom_output.contains("# HELP"));
    assert!(prom_output.contains("# TYPE"));

    assert_eq!(metrics_snap.dispatched, 65);
    assert_eq!(metrics_snap.suppressed, 5);
    assert!(metrics_snap.executed > 0);

    gateway.shutdown().await;
    println!("  [Scenario 5 passed]\n");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         MONITORING METRICS COLLECTION SIMULATION           ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    scenario_varied_traffic().await?;
    scenario_per_provider_metrics().await?;
    scenario_circuit_breaker_metrics().await?;
    scenario_periodic_snapshots().await?;
    scenario_prometheus_output().await?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
