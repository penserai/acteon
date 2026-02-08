//! Dashboard-style monitoring via SSE event streaming.
//!
//! This simulation demonstrates how an SSE subscriber can aggregate events
//! into real-time metrics, acting as a monitoring dashboard. It shows:
//!
//! 1. Aggregating events into per-provider and per-outcome counters
//! 2. Normal traffic patterns with healthy providers
//! 3. Provider failure scenario with circuit breaker events
//! 4. Mixed traffic with suppressed and rerouted actions
//! 5. Printing a dashboard-style summary of collected metrics
//!
//! Run with: `cargo run -p acteon-simulation --example stream_monitoring_simulation`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{CircuitBreakerConfig, GatewayBuilder};
use acteon_simulation::provider::{FailureMode, RecordingProvider};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
const SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-internal
    priority: 1
    condition:
      field: action.action_type
      eq: "internal_only"
    action:
      type: suppress
  - name: reroute-critical
    priority: 2
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: pagerduty
"#;

/// Simple metrics aggregator that mirrors what a monitoring dashboard would track.
struct DashboardMetrics {
    /// Total events received.
    total_events: usize,
    /// Events per provider.
    by_provider: HashMap<String, usize>,
    /// Events per outcome category.
    by_outcome: HashMap<String, usize>,
    /// Events per namespace.
    by_namespace: HashMap<String, usize>,
}

impl DashboardMetrics {
    fn new() -> Self {
        Self {
            total_events: 0,
            by_provider: HashMap::new(),
            by_outcome: HashMap::new(),
            by_namespace: HashMap::new(),
        }
    }

    fn ingest(&mut self, event: &StreamEvent) {
        self.total_events += 1;
        *self
            .by_namespace
            .entry(event.namespace.clone())
            .or_default() += 1;

        if let StreamEventType::ActionDispatched {
            ref outcome,
            ref provider,
        } = event.event_type
        {
            *self.by_provider.entry(provider.clone()).or_default() += 1;
            let category = acteon_core::outcome_category(outcome);
            *self.by_outcome.entry(category.to_string()).or_default() += 1;
        }
    }

    fn print_dashboard(&self, title: &str) {
        println!("  ┌──────────────────────────────────────────────────┐");
        println!("  │  {title:<48} │");
        println!("  ├──────────────────────────────────────────────────┤");
        println!("  │  Total events: {:<33} │", self.total_events);
        println!("  ├──────────────────────────────────────────────────┤");

        println!("  │  By Provider:                                    │");
        let mut providers: Vec<_> = self.by_provider.iter().collect();
        providers.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
        for (provider, count) in &providers {
            println!("  │    {provider:<20} {count:>6}                    │");
        }

        println!("  ├──────────────────────────────────────────────────┤");
        println!("  │  By Outcome:                                     │");
        let mut outcomes: Vec<_> = self.by_outcome.iter().collect();
        outcomes.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
        for (outcome, count) in &outcomes {
            println!("  │    {outcome:<20} {count:>6}                    │");
        }

        println!("  ├──────────────────────────────────────────────────┤");
        println!("  │  By Namespace:                                   │");
        let mut namespaces: Vec<_> = self.by_namespace.iter().collect();
        namespaces.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
        for (ns, count) in &namespaces {
            println!("  │    {ns:<20} {count:>6}                    │");
        }

        println!("  └──────────────────────────────────────────────────┘");
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║        STREAM MONITORING SIMULATION DEMO                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Normal Traffic Metrics
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: NORMAL TRAFFIC METRICS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Simulate healthy traffic across multiple providers and");
    println!("  aggregate events into dashboard metrics.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack"));
    let webhook = Arc::new(RecordingProvider::new("webhook"));

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .provider(Arc::clone(&email) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&slack) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&webhook) as Arc<dyn acteon_provider::DynProvider>)
        .build()?;

    let mut stream_rx = gateway.stream_tx().subscribe();
    let mut metrics = DashboardMetrics::new();

    // Generate normal traffic.
    let traffic = vec![
        ("notifications", "email", "send_email", 10),
        ("alerts", "slack", "post_alert", 5),
        ("integrations", "webhook", "fire_webhook", 3),
        ("billing", "email", "invoice", 7),
    ];

    for &(namespace, provider, action_type, count) in &traffic {
        for i in 0..count {
            let action = Action::new(
                namespace,
                "tenant-1",
                provider,
                action_type,
                serde_json::json!({"seq": i}),
            );
            gateway.dispatch(action, None).await?;
        }
    }

    let total_dispatched: usize = traffic.iter().map(|(_, _, _, c)| c).sum();
    println!("  Dispatched {total_dispatched} actions across 4 namespaces\n");

    // Ingest events into metrics.
    while let Ok(event) = stream_rx.try_recv() {
        metrics.ingest(&event);
    }

    metrics.print_dashboard("NORMAL TRAFFIC");
    assert_eq!(metrics.total_events, total_dispatched);

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Provider Failure with Circuit Breaker
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: PROVIDER FAILURE (CIRCUIT BREAKER)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A failing provider trips the circuit breaker. The monitoring");
    println!("  dashboard sees 'failed' then 'circuit_open' outcomes.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let healthy_provider = Arc::new(RecordingProvider::new("healthy-svc"));
    let failing_provider =
        Arc::new(RecordingProvider::new("failing-svc").with_failure_mode(FailureMode::Always));

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            ..ExecutorConfig::default()
        })
        .circuit_breaker(CircuitBreakerConfig {
            failure_threshold: 3,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(3600),
            fallback_provider: None,
        })
        .provider(Arc::clone(&healthy_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&failing_provider) as Arc<dyn acteon_provider::DynProvider>)
        .build()?;

    let mut stream_rx = gateway.stream_tx().subscribe();
    let mut metrics = DashboardMetrics::new();

    // Send healthy traffic.
    println!("  Phase 1: Healthy traffic (5 actions to healthy-svc)");
    for i in 0..5 {
        let action = Action::new(
            "monitoring",
            "tenant-1",
            "healthy-svc",
            "check",
            serde_json::json!({"seq": i}),
        );
        gateway.dispatch(action, None).await?;
    }

    // Send failing traffic to trip the circuit.
    println!("  Phase 2: Failing traffic (5 actions to failing-svc)");
    for i in 0..5 {
        let action = Action::new(
            "monitoring",
            "tenant-1",
            "failing-svc",
            "check",
            serde_json::json!({"seq": i}),
        );
        let outcome = gateway.dispatch(action, None).await?;
        println!("    Request {}: {}", i + 1, short_outcome(&outcome));
    }

    // Ingest events.
    while let Ok(event) = stream_rx.try_recv() {
        metrics.ingest(&event);
    }

    println!();
    metrics.print_dashboard("PROVIDER FAILURE SCENARIO");

    // We expect: 5 executed + 3 failed + 2 circuit_open = 10 total
    assert_eq!(metrics.total_events, 10);
    assert!(
        metrics.by_outcome.contains_key("executed"),
        "should have executed outcomes"
    );
    assert!(
        metrics.by_outcome.contains_key("failed")
            || metrics.by_outcome.contains_key("circuit_open"),
        "should have failure-related outcomes"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Mixed Traffic with Rules
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: MIXED TRAFFIC WITH RULE ENFORCEMENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Traffic includes actions that get suppressed, rerouted, and");
    println!("  executed normally. The dashboard tracks each outcome type.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let email = Arc::new(RecordingProvider::new("email"));
    let slack = Arc::new(RecordingProvider::new("slack"));
    let pagerduty = Arc::new(RecordingProvider::new("pagerduty"));

    let rules = {
        let frontend = acteon_rules_yaml::YamlFrontend;
        acteon_rules::RuleFrontend::parse(&frontend, SUPPRESSION_RULE)?
    };

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .rules(rules)
        .provider(Arc::clone(&email) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&slack) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&pagerduty) as Arc<dyn acteon_provider::DynProvider>)
        .build()?;

    let mut stream_rx = gateway.stream_tx().subscribe();
    let mut metrics = DashboardMetrics::new();

    // Normal actions.
    println!("  Dispatching 5 normal email actions...");
    for i in 0..5 {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"seq": i}),
        );
        gateway.dispatch(action, None).await?;
    }

    // Suppressed actions.
    println!("  Dispatching 3 internal_only actions (will be suppressed)...");
    for i in 0..3 {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "internal_only",
            serde_json::json!({"seq": i}),
        );
        gateway.dispatch(action, None).await?;
    }

    // Rerouted actions.
    println!("  Dispatching 2 critical-severity actions (will be rerouted to pagerduty)...");
    for i in 0..2 {
        let action = Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "alert",
            serde_json::json!({"severity": "critical", "seq": i}),
        );
        gateway.dispatch(action, None).await?;
    }

    // Ingest.
    while let Ok(event) = stream_rx.try_recv() {
        metrics.ingest(&event);
    }

    println!();
    metrics.print_dashboard("MIXED TRAFFIC WITH RULES");

    assert_eq!(metrics.total_events, 10);
    println!("\n  email calls: {}", email.call_count());
    println!("  slack calls: {}", slack.call_count());
    println!("  pagerduty calls: {}", pagerduty.call_count());

    gateway.shutdown().await;
    println!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Short outcome label.
fn short_outcome(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "EXECUTED",
        ActionOutcome::Failed(_) => "FAILED",
        ActionOutcome::Suppressed { .. } => "SUPPRESSED",
        ActionOutcome::Rerouted { .. } => "REROUTED",
        ActionOutcome::CircuitOpen { .. } => "CIRCUIT_OPEN",
        ActionOutcome::Deduplicated => "DEDUPLICATED",
        _ => "OTHER",
    }
}
