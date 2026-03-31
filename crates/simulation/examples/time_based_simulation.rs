//! Demonstration of time-based rule activation in the simulation framework.
//!
//! Time-based rules use `time.*` fields to match on the current UTC time at
//! dispatch. This enables patterns like business-hours suppression, weekend
//! rerouting, and off-hours throttling.
//!
//! Run with: cargo run -p acteon-simulation --example time_based_simulation

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use tracing::info;

// ---------------------------------------------------------------------------
// Rule definitions
// ---------------------------------------------------------------------------

/// Suppresses actions dispatched in years before 2025 (always passes in
/// practice — used here to verify temporal field evaluation works end-to-end
/// without depending on wall-clock time).
const YEAR_SUPPRESSION_RULE: &str = r#"
rules:
  - name: suppress-before-2025
    priority: 1
    description: "Suppress actions dispatched before 2025 (test rule)"
    condition:
      field: time.year
      lt: 2025
    action:
      type: suppress
"#;

/// Reroutes actions when the year is >= 2025 (always true in practice).
const YEAR_REROUTE_RULE: &str = r#"
rules:
  - name: reroute-modern-era
    priority: 1
    description: "Reroute to webhook when year >= 2025"
    condition:
      field: time.year
      gte: 2025
    action:
      type: reroute
      target_provider: webhook
"#;

/// Realistic business-hours suppression rule. Whether this fires depends on
/// the actual wall-clock time, so the demo prints the outcome without
/// asserting a specific result.
const BUSINESS_HOURS_RULE: &str = r#"
rules:
  - name: suppress-outside-business-hours
    priority: 1
    description: "Suppress email outside Mon-Fri 9-17 UTC"
    condition:
      any:
        - field: time.weekday_num
          gt: 5
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress
"#;

/// Combined rule: time condition AND action field condition.
const COMBINED_RULE: &str = r#"
rules:
  - name: suppress-old-email
    priority: 1
    description: "Suppress email before 2025 (always passes now)"
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - field: time.year
          lt: 2025
    action:
      type: suppress
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         TIME-BASED RULES SIMULATION DEMO                     ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Temporal Suppression (year-based, deterministic)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: TEMPORAL FIELD EVALUATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(YEAR_SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Hello"}),
    );

    let outcome = harness.dispatch(&action).await?;
    info!("  Rule: suppress-before-2025 (condition: time.year < 2025)");
    info!("  Outcome: {outcome:?}");
    // Current year is >= 2025, so the rule does NOT fire — action is executed.
    outcome.assert_executed();
    harness.provider("email").unwrap().assert_called(1);
    info!("  Result: Action executed (year >= 2025, rule did not match)");
    info!("  Provider calls: 1\n");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 2: Temporal Rerouting (year-based, deterministic)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: TEMPORAL REROUTING");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("webhook")
            .add_rule_yaml(YEAR_REROUTE_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    );

    let outcome = harness.dispatch(&action).await?;
    info!("  Rule: reroute-modern-era (condition: time.year >= 2025)");
    info!("  Outcome: {outcome:?}");
    outcome.assert_rerouted();
    harness.provider("email").unwrap().assert_not_called();
    harness.provider("webhook").unwrap().assert_called(1);
    info!("  Result: Rerouted from email to webhook");
    info!("  Email provider calls: 0");
    info!("  Webhook provider calls: 1\n");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 3: Business Hours Pattern (wall-clock dependent)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: BUSINESS HOURS SUPPRESSION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(BUSINESS_HOURS_RULE)
            .build(),
    )
    .await?;

    let now = chrono::Utc::now();
    info!(
        "  Current UTC time: {:02}:{:02} (weekday {})",
        now.format("%H"),
        now.format("%M"),
        now.format("%A")
    );
    info!("  Rule: suppress-outside-business-hours");
    info!("    Suppresses when: weekday_num > 5 OR hour < 9 OR hour >= 17\n");

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Report"}),
    );

    let outcome = harness.dispatch(&action).await?;
    info!("  Outcome: {outcome:?}");
    match &outcome {
        ActionOutcome::Executed(_) => {
            info!("  Result: Executed (within business hours)");
        }
        ActionOutcome::Suppressed { rule } => {
            info!("  Result: Suppressed by rule '{rule}' (outside business hours)");
        }
        other => {
            info!("  Result: Unexpected outcome: {other:?}");
        }
    }
    info!("");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 4: Combined Time + Action Conditions
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: COMBINED TIME + ACTION CONDITIONS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(COMBINED_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    );

    let outcome = harness.dispatch(&action).await?;
    info!("  Rule: suppress-old-email");
    info!("    Condition: action.action_type == 'send_email' AND time.year < 2025");
    info!("  Outcome: {outcome:?}");
    // Year is >= 2025, so the time condition fails and the action executes.
    outcome.assert_executed();
    info!("  Result: Executed (time.year >= 2025, combined condition not met)\n");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 5: Dry-Run with Time-Based Rules
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: DRY-RUN WITH TIME-BASED RULES");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("webhook")
            .add_rule_yaml(YEAR_REROUTE_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    );

    let outcome = harness.dispatch_dry_run(&action).await?;
    info!("  Rule: reroute-modern-era (condition: time.year >= 2025)");
    info!("  Outcome: {outcome:?}");
    outcome.assert_dry_run();

    match &outcome {
        ActionOutcome::DryRun {
            verdict,
            matched_rule,
            would_be_provider,
        } => {
            info!("  Verdict: {verdict}");
            info!("  Matched rule: {matched_rule:?}");
            info!("  Would-be provider: {would_be_provider}");
            assert_eq!(verdict, "reroute");
        }
        _ => panic!("expected DryRun outcome"),
    }

    // Verify no providers were actually called.
    harness.provider("email").unwrap().assert_not_called();
    harness.provider("webhook").unwrap().assert_not_called();
    info!("  Provider calls: 0 (dry-run skips execution)\n");

    harness.teardown().await?;

    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║  ALL DEMOS COMPLETED SUCCESSFULLY                            ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
