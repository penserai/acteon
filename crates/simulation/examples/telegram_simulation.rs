//! Telegram Bot provider simulation scenarios.
//!
//! Demonstrates dispatching messages through the Telegram provider:
//! a plain-text deploy notification, an HTML-formatted alert with
//! a thread id targeting a forum-group topic, and a rule-based
//! reroute that sends high-priority events to Telegram.
//!
//! The scenarios use the simulation harness' recording provider
//! named `"telegram"`, so no real Telegram bot credentials are
//! needed.
//!
//! Run with: `cargo run -p acteon-simulation --example telegram_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

const REROUTE_HIGH_TO_TELEGRAM_RULE: &str = r#"
rules:
  - name: reroute-high-to-telegram
    priority: 1
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: telegram
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          TELEGRAM PROVIDER SIMULATION DEMO                  ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Plain-text notification
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: PLAIN-TEXT DEPLOY NOTIFICATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("telegram")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'telegram' recording provider\n");

    let deploy = Action::new(
        "notifications",
        "tenant-1",
        "telegram",
        "notify",
        serde_json::json!({
            "text": "Deploy #4823 completed successfully.",
            "chat": "ops-channel",
        }),
    );
    info!("→ Dispatching plain-text deploy notification...");
    let outcome = harness.dispatch(&deploy).await?;
    info!("  Outcome: {outcome:?}\n");

    // =========================================================================
    // DEMO 2: HTML-formatted alert with topic thread
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: HTML-FORMATTED ALERT (with forum-group thread)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let alert = Action::new(
        "incidents",
        "tenant-1",
        "telegram",
        "notify",
        serde_json::json!({
            "text": "<b>CRITICAL</b>: checkout-api error rate above SLO.\n<i>Investigate immediately.</i>",
            "chat": "ops-channel",
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
            "protect_content": true,
            "message_thread_id": 7,
        }),
    );
    info!("→ Dispatching HTML alert to topic thread 7...");
    let outcome = harness.dispatch(&alert).await?;
    info!("  Outcome: {outcome:?}");

    let provider = harness.provider("telegram").unwrap();
    info!(
        "\n  Telegram provider received {} message(s)",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 2);
    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Rule-based reroute of critical severity to Telegram
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: RULE-BASED REROUTE — severity=critical → Telegram");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_recording_provider("telegram")
            .add_rule_yaml(REROUTE_HIGH_TO_TELEGRAM_RULE)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with log + telegram recording providers");
    info!("✓ Loaded reroute rule: severity=critical → telegram\n");

    // Warning severity — stays on log.
    let warn = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "severity": "warning",
            "text": "Queue depth above warning threshold",
        }),
    );
    info!("→ Dispatching severity=warning (should stay on 'log')...");
    let outcome = harness.dispatch(&warn).await?;
    info!("  Outcome: {outcome:?}");

    // Critical severity — routed to telegram.
    let critical = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "severity": "critical",
            "text": "checkout-api is down",
        }),
    );
    info!("→ Dispatching severity=critical (should reroute to 'telegram')...");
    let outcome = harness.dispatch(&critical).await?;
    info!("  Outcome: {outcome:?}");

    let log_calls = harness.provider("log").unwrap().call_count();
    let telegram_calls = harness.provider("telegram").unwrap().call_count();
    info!("\n  Provider call counts:");
    info!("    log:      {log_calls}");
    info!("    telegram: {telegram_calls}");
    assert_eq!(log_calls, 1);
    assert_eq!(telegram_calls, 1);

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
