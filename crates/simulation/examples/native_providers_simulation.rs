//! Simulation of the three native providers: Twilio, Teams, and Discord.
//!
//! Demonstrates dispatching actions with the correct payload shapes for each
//! provider and validates that all dispatches succeed.
//!
//! Run with: cargo run -p acteon-simulation --example native_providers_simulation

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("══════════════════════════════════════════════════════════════");
    println!("       NATIVE PROVIDERS SIMULATION");
    println!("══════════════════════════════════════════════════════════════\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("twilio")
            .add_recording_provider("teams")
            .add_recording_provider("discord")
            .build(),
    )
    .await?;

    println!("Started simulation cluster with 1 node");
    println!("Registered providers: twilio, teams, discord\n");

    let mut results: Vec<(&str, ActionOutcome)> = Vec::new();

    // =========================================================================
    // Twilio SMS
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  TWILIO SMS");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching SMS to +15559876543...");
    let outcome = harness.dispatch(&twilio_action).await?;
    println!("  Action ID: {}", twilio_action.id);
    println!("  Outcome:   {:?}", outcome);
    println!(
        "  Provider called: {} time(s)\n",
        harness.provider("twilio").unwrap().call_count()
    );
    results.push(("twilio", outcome));

    // =========================================================================
    // Microsoft Teams
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  MICROSOFT TEAMS");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching Teams message...");
    let outcome = harness.dispatch(&teams_action).await?;
    println!("  Action ID: {}", teams_action.id);
    println!("  Outcome:   {:?}", outcome);
    println!(
        "  Provider called: {} time(s)\n",
        harness.provider("teams").unwrap().call_count()
    );
    results.push(("teams", outcome));

    // =========================================================================
    // Discord
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DISCORD");
    println!("------------------------------------------------------------------\n");

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

    println!("  Dispatching Discord message with embed...");
    let outcome = harness.dispatch(&discord_action).await?;
    println!("  Action ID: {}", discord_action.id);
    println!("  Outcome:   {:?}", outcome);
    println!(
        "  Provider called: {} time(s)\n",
        harness.provider("discord").unwrap().call_count()
    );
    results.push(("discord", outcome));

    // =========================================================================
    // Summary
    // =========================================================================
    println!("══════════════════════════════════════════════════════════════");
    println!("  SUMMARY");
    println!("══════════════════════════════════════════════════════════════\n");

    let mut all_passed = true;
    for (name, outcome) in &results {
        let passed = matches!(outcome, ActionOutcome::Executed(_));
        let status = if passed { "PASS" } else { "FAIL" };
        println!("  [{status}] {name}: {outcome:?}");
        if !passed {
            all_passed = false;
        }
    }

    println!();
    println!(
        "  Total dispatched: {}  |  Twilio calls: {}  |  Teams calls: {}  |  Discord calls: {}",
        results.len(),
        harness.provider("twilio").unwrap().call_count(),
        harness.provider("teams").unwrap().call_count(),
        harness.provider("discord").unwrap().call_count(),
    );

    harness.teardown().await?;
    println!("\n  Simulation cluster shut down");

    if all_passed {
        println!("\n  All providers dispatched successfully.");
    } else {
        println!("\n  Some providers failed. See details above.");
        std::process::exit(1);
    }

    Ok(())
}
