//! Pushover provider simulation scenarios.
//!
//! Demonstrates dispatching push notifications through the
//! Pushover provider: a normal-priority deploy notification, an
//! emergency-priority alert that includes `retry`/`expire`, and a
//! rule-based reroute sending high-priority events to Pushover.
//!
//! The scenarios use the simulation harness' recording provider
//! named `"pushover"`, so no real Pushover credentials are needed.
//!
//! Run with: `cargo run -p acteon-simulation --example pushover_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

const REROUTE_HIGH_TO_PUSHOVER_RULE: &str = r#"
rules:
  - name: reroute-high-to-pushover
    priority: 1
    condition:
      field: action.payload.priority
      gte: 1
    action:
      type: reroute
      target_provider: pushover
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          PUSHOVER PROVIDER SIMULATION DEMO                  ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Normal-priority notification
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: NORMAL-PRIORITY NOTIFICATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("pushover")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'pushover' recording provider\n");

    let deploy = Action::new(
        "notifications",
        "tenant-1",
        "pushover",
        "notify",
        serde_json::json!({
            "message": "Deploy #4823 completed successfully.",
            "title": "CI/CD — production",
            "priority": 0,
            "url": "https://ci.example.com/build/4823",
            "url_title": "View build",
        }),
    );
    info!("→ Dispatching normal-priority deploy notification...");
    let outcome = harness.dispatch(&deploy).await?;
    info!("  Outcome: {outcome:?}\n");

    // =========================================================================
    // DEMO 2: Emergency-priority alert (requires retry + expire)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: EMERGENCY PRIORITY (pages until acknowledged)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let emergency = Action::new(
        "incidents",
        "tenant-1",
        "pushover",
        "notify",
        serde_json::json!({
            "message": "Checkout API is returning 5xx for >50% of traffic.",
            "title": "CRITICAL: checkout-api down",
            "priority": 2,
            "retry": 60,       // re-notify every 60s...
            "expire": 3600,    // ...for up to 1 hour or until ack
            "sound": "siren",
            "url": "https://wiki.example.com/runbook/checkout-5xx",
            "url_title": "Open runbook",
        }),
    );
    info!("→ Dispatching emergency-priority alert (retry=60s, expire=1h)...");
    let outcome = harness.dispatch(&emergency).await?;
    info!("  Outcome: {outcome:?}");

    let provider = harness.provider("pushover").unwrap();
    info!(
        "\n  Pushover provider received {} notification(s)",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 2);
    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Rule-based reroute — high-priority events go to Pushover
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: RULE-BASED REROUTE — priority>=1 → Pushover");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_recording_provider("pushover")
            .add_rule_yaml(REROUTE_HIGH_TO_PUSHOVER_RULE)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with log + pushover recording providers");
    info!("✓ Loaded reroute rule: priority>=1 → pushover\n");

    // Low-priority — stays on log.
    let low = Action::new(
        "notifications",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "message": "Nightly index rebuild started.",
            "priority": 0,
        }),
    );
    info!("→ Dispatching priority=0 (should stay on 'log')...");
    let outcome = harness.dispatch(&low).await?;
    info!("  Outcome: {outcome:?}");

    // High-priority — routed to pushover.
    let high = Action::new(
        "notifications",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "message": "Disk usage above 85% on db-primary-01.",
            "title": "Disk pressure",
            "priority": 1,
        }),
    );
    info!("→ Dispatching priority=1 (should reroute to 'pushover')...");
    let outcome = harness.dispatch(&high).await?;
    info!("  Outcome: {outcome:?}");

    let log_calls = harness.provider("log").unwrap().call_count();
    let pushover_calls = harness.provider("pushover").unwrap().call_count();
    info!("\n  Provider call counts:");
    info!("    log:      {log_calls}");
    info!("    pushover: {pushover_calls}");
    assert_eq!(log_calls, 1);
    assert_eq!(pushover_calls, 1);

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
