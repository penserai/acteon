//! Simulation of the three native providers: Twilio, Teams, and Discord.
//!
//! Demonstrates dispatching actions with the correct payload shapes for each
//! provider and validates that all dispatches succeed.
//!
//! Run with: cargo run -p acteon-simulation --example native_providers_simulation

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("══════════════════════════════════════════════════════════════");
    info!("       NATIVE PROVIDERS SIMULATION");
    info!("══════════════════════════════════════════════════════════════\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("twilio")
            .add_recording_provider("teams")
            .add_recording_provider("discord")
            .build(),
    )
    .await?;

    info!("Started simulation cluster with 1 node");
    info!("Registered providers: twilio, teams, discord\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // Twilio SMS
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  TWILIO SMS");
    info!("------------------------------------------------------------------\n");

    let twilio_action = Action::new(
        "notifications",
        "acme-corp",
        "twilio",
        "send_sms",
        serde_json::json!({
            "to": "+15559876543",
            "body": "Server alert!",
            "from": "+15551234567"
        }),
    );

    info!("  Dispatching SMS to +15559876543...");
    let outcome = harness.dispatch(&twilio_action).await?;
    info!("  Action ID: {}", twilio_action.id);
    info!("  Outcome:   {:?}", outcome);
    info!(
        "  Provider called: {} time(s)\n",
        harness.provider("twilio").unwrap().call_count()
    );
    results.push(("twilio", outcome));

    // =========================================================================
    // Microsoft Teams
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  MICROSOFT TEAMS");
    info!("------------------------------------------------------------------\n");

    let teams_action = Action::new(
        "notifications",
        "acme-corp",
        "teams",
        "send_message",
        serde_json::json!({
            "text": "Deployment complete",
            "title": "CI/CD",
            "theme_color": "00FF00"
        }),
    );

    info!("  Dispatching Teams message...");
    let outcome = harness.dispatch(&teams_action).await?;
    info!("  Action ID: {}", teams_action.id);
    info!("  Outcome:   {:?}", outcome);
    info!(
        "  Provider called: {} time(s)\n",
        harness.provider("teams").unwrap().call_count()
    );
    results.push(("teams", outcome));

    // =========================================================================
    // Discord
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DISCORD");
    info!("------------------------------------------------------------------\n");

    let discord_action = Action::new(
        "notifications",
        "acme-corp",
        "discord",
        "send_message",
        serde_json::json!({
            "content": "Build passed!",
            "embeds": [
                {
                    "title": "Build #42",
                    "description": "All tests passed",
                    "color": 65280
                }
            ]
        }),
    );

    info!("  Dispatching Discord message with embed...");
    let outcome = harness.dispatch(&discord_action).await?;
    info!("  Action ID: {}", discord_action.id);
    info!("  Outcome:   {:?}", outcome);
    info!(
        "  Provider called: {} time(s)\n",
        harness.provider("discord").unwrap().call_count()
    );
    results.push(("discord", outcome));

    // =========================================================================
    // Summary
    // =========================================================================
    info!("══════════════════════════════════════════════════════════════");
    info!("  SUMMARY");
    info!("══════════════════════════════════════════════════════════════\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        info!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    info!("");
    info!(
        "  Total dispatched: {}  |  Twilio calls: {}  |  Teams calls: {}  |  Discord calls: {}",
        results.len(),
        harness.provider("twilio").unwrap().call_count(),
        harness.provider("teams").unwrap().call_count(),
        harness.provider("discord").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n  Simulation cluster shut down");

    if all_passed {
        info!("\n  All providers dispatched successfully.");
    } else {
        info!("\n  Some providers failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
