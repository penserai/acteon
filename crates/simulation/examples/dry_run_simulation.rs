//! Demonstration of dry-run mode in the simulation framework.
//!
//! Dry-run evaluates the full rule pipeline and returns what *would* happen
//! without executing the action, recording state, or emitting audit records.
//!
//! Run with: cargo run -p acteon-simulation --example dry_run_simulation

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use tracing::info;

const SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
"#;

const REROUTE_RULE: &str = r#"
rules:
  - name: reroute-urgent
    priority: 1
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: sms
"#;

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 2
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║             DRY-RUN MODE SIMULATION DEMO                     ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Dry-Run Allow Verdict
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: DRY-RUN ALLOW VERDICT");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    // Normal action with no matching suppression rule -> would be allowed.
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Hello"}),
    );

    let outcome = harness.dispatch_dry_run(&action).await?;
    info!("  Action: send_email via email provider");
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
            assert_eq!(verdict, "allow");
            assert!(matched_rule.is_none());
            assert_eq!(would_be_provider, "email");
        }
        _ => panic!("expected DryRun"),
    }

    // Provider was NOT called.
    harness.provider("email").unwrap().assert_not_called();
    info!("  Provider calls: 0 (dry-run skips execution)");
    info!("");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 2: Dry-Run Suppression Verdict
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: DRY-RUN SUPPRESSION VERDICT");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    // This action matches the "block-spam" rule -> would be suppressed.
    let spam_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({"body": "buy now!"}),
    );

    let outcome = harness.dispatch_dry_run(&spam_action).await?;
    info!("  Action: spam via email provider");
    info!("  Outcome: {outcome:?}");
    outcome.assert_dry_run();

    match &outcome {
        ActionOutcome::DryRun {
            verdict,
            matched_rule,
            ..
        } => {
            info!("  Verdict: {verdict}");
            info!("  Matched rule: {matched_rule:?}");
            assert_eq!(verdict, "suppress");
            assert_eq!(matched_rule.as_deref(), Some("block-spam"));
        }
        _ => panic!("expected DryRun"),
    }

    harness.provider("email").unwrap().assert_not_called();
    info!("  Provider calls: 0 (dry-run skips execution)");
    info!("");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 3: Dry-Run Reroute Verdict
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: DRY-RUN REROUTE VERDICT");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(REROUTE_RULE)
            .build(),
    )
    .await?;

    // Urgent action -> would be rerouted from email to sms.
    let urgent_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "alert",
        serde_json::json!({"priority": "urgent", "message": "Server is down!"}),
    );

    let outcome = harness.dispatch_dry_run(&urgent_action).await?;
    info!("  Action: alert via email provider (priority=urgent)");
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
            info!("  Would-be provider: {would_be_provider} (rerouted from email)");
            assert_eq!(verdict, "reroute");
            assert_eq!(matched_rule.as_deref(), Some("reroute-urgent"));
            assert_eq!(would_be_provider, "sms");
        }
        _ => panic!("expected DryRun"),
    }

    // Neither provider was called.
    harness.provider("email").unwrap().assert_not_called();
    harness.provider("sms").unwrap().assert_not_called();
    info!("  Email provider calls: 0");
    info!("  SMS provider calls: 0");
    info!("");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 4: Dry-Run vs Normal Dispatch Comparison
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: DRY-RUN vs NORMAL DISPATCH COMPARISON");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "notify",
        serde_json::json!({"msg": "hello"}),
    )
    .with_dedup_key("dedup-key-1");

    // Dry-run: shows "deduplicate" verdict, but does NOT record the dedup key.
    let dry_outcome = harness.dispatch_dry_run(&action).await?;
    info!("  Dry-run outcome: {dry_outcome:?}");
    dry_outcome.assert_dry_run();
    match &dry_outcome {
        ActionOutcome::DryRun { verdict, .. } => {
            info!("  Verdict: {verdict}");
            assert_eq!(verdict, "deduplicate");
        }
        _ => panic!("expected DryRun"),
    }
    harness.provider("email").unwrap().assert_not_called();
    info!("  Provider calls after dry-run: 0");

    // Normal dispatch: the action is actually executed (first time with this dedup key).
    let normal_outcome = harness.dispatch(&action).await?;
    info!("  Normal outcome: {normal_outcome:?}");
    normal_outcome.assert_executed();
    harness.provider("email").unwrap().assert_called(1);
    info!("  Provider calls after normal dispatch: 1");

    // Second normal dispatch: now deduplicated because the key was recorded.
    let dedup_outcome = harness.dispatch(&action).await?;
    info!("  Second dispatch outcome: {dedup_outcome:?}");
    dedup_outcome.assert_deduplicated();
    harness.provider("email").unwrap().assert_called(1);
    info!("  Provider calls (still 1, second was deduped): 1");
    info!("");

    harness.teardown().await?;

    // =========================================================================
    // DEMO 5: Batch Dry-Run
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: BATCH DRY-RUN");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(SUPPRESSION_RULE)
            .add_rule_yaml(REROUTE_RULE)
            .build(),
    )
    .await?;

    let actions = vec![
        // Would be allowed (no matching rule).
        Action::new(
            "ns",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({}),
        ),
        // Would be suppressed by "block-spam".
        Action::new("ns", "tenant-1", "email", "spam", serde_json::json!({})),
        // Would be rerouted from email to sms by "reroute-urgent".
        Action::new(
            "ns",
            "tenant-1",
            "email",
            "alert",
            serde_json::json!({"priority": "urgent"}),
        ),
    ];

    let outcomes = harness.dispatch_batch_dry_run(&actions).await;
    info!("  Batch size: {}", actions.len());
    for (i, outcome) in outcomes.iter().enumerate() {
        let outcome = outcome.as_ref().unwrap();
        info!("  Action {i}: {outcome:?}");
        outcome.assert_dry_run();
    }

    // No providers were called.
    harness.provider("email").unwrap().assert_not_called();
    harness.provider("sms").unwrap().assert_not_called();
    info!("  Total provider calls: 0 (batch dry-run)");
    info!("");

    harness.teardown().await?;

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║                  ALL DEMOS PASSED                            ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
