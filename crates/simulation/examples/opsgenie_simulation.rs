//! OpsGenie provider simulation scenarios.
//!
//! Demonstrates dispatching alerts to the OpsGenie provider through
//! the standard create → acknowledge → close lifecycle, and shows a
//! rule-based reroute pattern for steering high-priority alerts to
//! the OpsGenie integration.
//!
//! These scenarios use the simulation harness' recording provider
//! named `"opsgenie"`, so no real OpsGenie API credentials are
//! needed — the harness captures the dispatched actions and asserts
//! that they landed on the right provider with the right payload.
//!
//! Run with: `cargo run -p acteon-simulation --example opsgenie_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

const REROUTE_TO_OPSGENIE_RULE: &str = r#"
rules:
  - name: reroute-p1-to-opsgenie
    priority: 1
    condition:
      field: action.payload.priority
      eq: "P1"
    action:
      type: reroute
      target_provider: opsgenie
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          OPSGENIE PROVIDER SIMULATION DEMO                  ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Alert lifecycle (create → acknowledge → close)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: ALERT LIFECYCLE (create → acknowledge → close)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("opsgenie")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'opsgenie' recording provider\n");

    // 1. CREATE — a Prometheus-style alert fires and Acteon dispatches
    //    it to OpsGenie via the native provider type.
    let create = Action::new(
        "incidents",
        "tenant-1",
        "opsgenie",
        "send_alert",
        serde_json::json!({
            "event_action": "create",
            "message": "High error rate on checkout-api",
            "alias": "checkout-api-5xx",
            "description": "5xx rate has crossed the SLO threshold for 5 minutes.",
            "priority": "P2",
            "tags": ["checkout", "5xx", "slo-breach"],
            "responders": [
                {"name": "checkout-oncall", "type": "team"}
            ],
            "details": {
                "runbook": "https://wiki.example.com/runbook/checkout-5xx",
                "service": "checkout-api",
                "env": "production"
            },
            "source": "prometheus"
        }),
    );
    info!("→ Dispatching CREATE alert (alias=checkout-api-5xx, priority=P2)...");
    let outcome = harness.dispatch(&create).await?;
    info!("  Outcome: {outcome:?}");

    // 2. ACKNOWLEDGE — the on-call engineer picks up the alert.
    let ack = Action::new(
        "incidents",
        "tenant-1",
        "opsgenie",
        "send_alert",
        serde_json::json!({
            "event_action": "acknowledge",
            "alias": "checkout-api-5xx",
            "user": "oncall-alice",
            "note": "Investigating — rolled back deploy #4823"
        }),
    );
    info!("→ Dispatching ACKNOWLEDGE (oncall picked up)...");
    let outcome = harness.dispatch(&ack).await?;
    info!("  Outcome: {outcome:?}");

    // 3. CLOSE — the underlying issue is resolved.
    let close = Action::new(
        "incidents",
        "tenant-1",
        "opsgenie",
        "send_alert",
        serde_json::json!({
            "event_action": "close",
            "alias": "checkout-api-5xx",
            "user": "oncall-alice",
            "note": "Rollback confirmed; 5xx rate back below threshold."
        }),
    );
    info!("→ Dispatching CLOSE (incident resolved)...");
    let outcome = harness.dispatch(&close).await?;
    info!("  Outcome: {outcome:?}");

    let provider = harness.provider("opsgenie").unwrap();
    info!(
        "\n  OpsGenie provider received {} action(s) across the lifecycle",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 3);
    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Rule-based reroute of P1 alerts to OpsGenie
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: RULE-BASED REROUTE — P1 alerts land on OpsGenie");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_recording_provider("opsgenie")
            .add_rule_yaml(REROUTE_TO_OPSGENIE_RULE)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with log + opsgenie recording providers");
    info!("✓ Loaded reroute rule: P1 priority → opsgenie\n");

    // Non-P1 alert — stays on the log provider.
    let p3 = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "message": "Queue depth above warning threshold",
            "priority": "P3",
        }),
    );
    info!("→ Dispatching P3 alert (should stay on 'log')...");
    let outcome = harness.dispatch(&p3).await?;
    info!("  Outcome: {outcome:?}");

    // P1 alert — the rule reroutes it to opsgenie.
    let p1 = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "event_action": "create",
            "message": "Checkout-api is down",
            "alias": "checkout-api-down",
            "priority": "P1",
        }),
    );
    info!("→ Dispatching P1 alert (should reroute to 'opsgenie')...");
    let outcome = harness.dispatch(&p1).await?;
    info!("  Outcome: {outcome:?}");

    let log_calls = harness.provider("log").unwrap().call_count();
    let opsgenie_calls = harness.provider("opsgenie").unwrap().call_count();
    info!("\n  Provider call counts:");
    info!("    log:      {log_calls}");
    info!("    opsgenie: {opsgenie_calls}");
    assert_eq!(log_calls, 1, "P3 should have stayed on log");
    assert_eq!(opsgenie_calls, 1, "P1 should have been rerouted");

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
