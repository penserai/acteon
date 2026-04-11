//! WeChat Work (企业微信) provider simulation scenarios.
//!
//! Demonstrates dispatching messages through the WeChat provider:
//! a plain-text deploy notification broadcast to the agent's
//! entire audience (`@all`), a markdown alert targeting a specific
//! department, a textcard alert with a runbook link, and a
//! rule-based reroute for tenant-critical alerts.
//!
//! The scenarios use the simulation harness' recording provider
//! named `"wechat"`, so no real WeChat Work credentials are
//! needed — the harness captures the dispatched actions and
//! verifies they land on the right provider.
//!
//! Run with: `cargo run -p acteon-simulation --example wechat_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

const REROUTE_TENANT_CRITICAL_TO_WECHAT_RULE: &str = r#"
rules:
  - name: reroute-tenant-critical-to-wechat
    priority: 1
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: wechat
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          WECHAT PROVIDER SIMULATION DEMO                    ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Text broadcast to @all
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: PLAIN TEXT TO @all");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("wechat")
            .build(),
    )
    .await?;
    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'wechat' recording provider\n");

    let broadcast = Action::new(
        "notifications",
        "tenant-1",
        "wechat",
        "notify",
        serde_json::json!({
            "touser": "@all",
            "msgtype": "text",
            "content": "Deploy #4823 shipped to production.",
        }),
    );
    info!("→ Dispatching text broadcast to @all...");
    let outcome = harness.dispatch(&broadcast).await?;
    info!("  Outcome: {outcome:?}\n");

    // =========================================================================
    // DEMO 2: Markdown to a specific department
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: MARKDOWN TO DEPARTMENT (toparty)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let markdown_alert = Action::new(
        "incidents",
        "tenant-1",
        "wechat",
        "notify",
        serde_json::json!({
            "toparty": "12|15",
            "msgtype": "markdown",
            "content": "### Latency spike on **checkout-api**\n> p95 above 2s for 5 minutes\n> [Open runbook](https://wiki.example.com/runbook/checkout-latency)",
        }),
    );
    info!("→ Dispatching markdown alert to toparty=12|15...");
    let outcome = harness.dispatch(&markdown_alert).await?;
    info!("  Outcome: {outcome:?}\n");

    // =========================================================================
    // DEMO 3: Textcard with a runbook link
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: TEXTCARD WITH RUNBOOK LINK");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let textcard = Action::new(
        "incidents",
        "tenant-1",
        "wechat",
        "notify",
        serde_json::json!({
            "totag": "oncall",
            "msgtype": "textcard",
            "title": "CRITICAL: checkout-api down",
            "description": "5xx rate above 50% for 2 minutes. Oncall paged.",
            "url": "https://wiki.example.com/runbook/checkout-5xx",
            "btntxt": "Open runbook",
        }),
    );
    info!("→ Dispatching textcard to totag=oncall...");
    let outcome = harness.dispatch(&textcard).await?;
    info!("  Outcome: {outcome:?}");

    let provider = harness.provider("wechat").unwrap();
    info!(
        "\n  WeChat provider received {} message(s)",
        provider.call_count()
    );
    assert_eq!(provider.call_count(), 3);
    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 4: Rule-based reroute of critical severity to WeChat
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: RULE-BASED REROUTE — severity=critical → WeChat");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("log")
            .add_recording_provider("wechat")
            .add_rule_yaml(REROUTE_TENANT_CRITICAL_TO_WECHAT_RULE)
            .build(),
    )
    .await?;
    info!("✓ Started cluster with log + wechat recording providers");
    info!("✓ Loaded reroute rule: severity=critical → wechat\n");

    // Warning severity — stays on log.
    let warn = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "severity": "warning",
            "content": "queue depth above threshold",
        }),
    );
    info!("→ Dispatching severity=warning (should stay on 'log')...");
    let outcome = harness.dispatch(&warn).await?;
    info!("  Outcome: {outcome:?}");

    // Critical severity — routed to wechat.
    let critical = Action::new(
        "incidents",
        "tenant-1",
        "log",
        "notify",
        serde_json::json!({
            "severity": "critical",
            "touser": "@all",
            "msgtype": "text",
            "content": "checkout-api is down",
        }),
    );
    info!("→ Dispatching severity=critical (should reroute to 'wechat')...");
    let outcome = harness.dispatch(&critical).await?;
    info!("  Outcome: {outcome:?}");

    let log_calls = harness.provider("log").unwrap().call_count();
    let wechat_calls = harness.provider("wechat").unwrap().call_count();
    info!("\n  Provider call counts:");
    info!("    log:    {log_calls}");
    info!("    wechat: {wechat_calls}");
    assert_eq!(log_calls, 1);
    assert_eq!(wechat_calls, 1);

    harness.teardown().await?;
    info!("\n✓ All demos complete.");
    Ok(())
}
