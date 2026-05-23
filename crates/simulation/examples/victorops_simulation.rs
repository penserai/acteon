//! VictorOps (Splunk On-Call) provider simulation scenarios.
//!
//! Demonstrates dispatching alerts through the full trigger → ack
//! → resolve lifecycle and a rule-based CRITICAL reroute pattern.
//!
//! The scenarios use the simulation harness' recording provider
//! named `"victorops"`, so no real VictorOps API credentials are
//! needed — the harness captures the dispatched actions and
//! asserts that they landed on the right provider.
//!
//! Run with: `cargo run -p acteon-simulation --example victorops_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

const REROUTE_TO_VICTOROPS_RULE: &str = r#"
rules:
  - name: reroute-critical-to-victorops
    priority: 1
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: victorops
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         VICTOROPS PROVIDER SIMULATION DEMO                  ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Alert lifecycle (trigger → acknowledge → resolve)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: ALERT LIFECYCLE (trigger → acknowledge → resolve)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("victorops")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'victorops' recording provider\n");

    // 1. TRIGGER — a Prometheus-style alert fires and Acteon
    //    dispatches it to VictorOps with message_type=CRITICAL.
    let trigger = Action::new(
        "incidents",
        "tenant-1",
        "victorops",
        "send_alert",
        serde_json::json!({
            "event_action": "trigger",
            "entity_id": "checkout-api-5xx",
            "entity_display_name": "Checkout API 5xx rate above SLO",
            "state_message": "5xx rate crossed SLO threshold for 5 minutes.",
            "host_name": "checkout-api",
            "routing_key": "team-ops",
        }),
    );
    info!("→ Dispatching TRIGGER (entity_id=checkout-api-5xx, CRITICAL)...");
    let outcome = harness.dispatch(&trigger).await?;
    info!("  Outcome: {outcome:?}");

    // 2. ACKNOWLEDGE — the on-call engineer picks up the alert.
    let ack = Action::new(
        "incidents",
        "tenant-1",
        "victorops",
        "send_alert",
        serde_json::json!({
            "event_action": "acknowledge",
            "entity_id": "checkout-api-5xx",
            "state_message": "Investigating — rolling back deploy #4823.",
        }),
    );
    info!("→ Dispatching ACKNOWLEDGE (oncall picked up)...");
    let outcome = harness.dispatch(&ack).await?;
    info!("  Outcome: {outcome:?}");

    // 3. RESOLVE — the underlying issue is cleared.
    let resolve = Action::new(
        "incidents",
        "tenant-1",
        "victorops",
        "send_alert",
        serde_json::json!({
            "event_action": "resolve",
            "entity_id": "checkout-api-5xx",
            "state_message": "Rollback confirmed; error rate back below threshold.",
        }),
    );
    info!("→ Dispatching RESOLVE (incident closed)...");
    let outcome = harness.dispatch(&resolve).await?;
    info!("  Outcome: {outcome:?}");

    let provider = harness.provider("victorops").unwrap();
    info!(
        "\n  VictorOps provider received {} action(s) across the lifecycle",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 3);
    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Rule-based reroute of critical alerts to VictorOps
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: RULE-BASED REROUTE — critical alerts → VictorOps");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_recording_provider("victorops")
            .add_rule_yaml(REROUTE_TO_VICTOROPS_RULE)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with log + victorops recording providers");
    info!("✓ Loaded reroute rule: severity=critical → victorops\n");

    // Non-critical alert — stays on the log provider.
    let warn = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "severity": "warning",
            "message": "Queue depth above warning threshold",
        }),
    );
    info!("→ Dispatching WARN alert (should stay on 'log')...");
    let outcome = harness.dispatch(&warn).await?;
    info!("  Outcome: {outcome:?}");

    // Critical alert — the rule reroutes it to victorops.
    let critical = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "event_action": "trigger",
            "entity_id": "checkout-api-down",
            "severity": "critical",
            "entity_display_name": "Checkout API is down",
        }),
    );
    info!("→ Dispatching CRITICAL alert (should reroute to 'victorops')...");
    let outcome = harness.dispatch(&critical).await?;
    info!("  Outcome: {outcome:?}");

    let log_calls = harness.provider("log").unwrap().call_count();
    let victorops_calls = harness.provider("victorops").unwrap().call_count();
    info!("\n  Provider call counts:");
    info!("    log:       {log_calls}");
    info!("    victorops: {victorops_calls}");
    assert_eq!(log_calls, 1, "warn alert should have stayed on log");
    assert_eq!(
        victorops_calls, 1,
        "critical alert should have been rerouted"
    );

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
