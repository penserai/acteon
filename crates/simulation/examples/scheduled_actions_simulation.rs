//! Demonstration of Delayed/Scheduled Actions in the simulation framework.
//!
//! This example shows how rules with `type: schedule` defer action execution
//! by persisting the action and scheduling it for later dispatch. The gateway
//! returns `ActionOutcome::Scheduled` immediately, and a background processor
//! picks up due actions when their scheduled time arrives.
//!
//! Scenarios demonstrated:
//!   1. Delayed email reminder (welcome email scheduled after sign-up)
//!   2. Off-peak retry (failed action rescheduled to quiet hours)
//!   3. Escalation workflow (alert with delayed escalation)
//!   4. Multi-tenant scheduling (independent delays per tenant)
//!   5. Batch processing window (small actions batched via scheduling)
//!
//! Run with: `cargo run -p acteon-simulation --example scheduled_actions_simulation`

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;

// =============================================================================
// Rule definitions
// =============================================================================

/// Schedule a welcome email 24 hours (86400s) after user sign-up.
const WELCOME_EMAIL_RULE: &str = r#"
rules:
  - name: schedule-welcome-email
    priority: 10
    description: "Schedule welcome email 24h after sign-up"
    condition:
      field: action.action_type
      eq: "welcome_email"
    action:
      type: schedule
      delay_seconds: 86400
"#;

/// Schedule a cart-abandonment reminder 1 hour (3600s) after the event.
const CART_REMINDER_RULE: &str = r#"
rules:
  - name: schedule-cart-reminder
    priority: 10
    description: "Schedule cart abandonment reminder in 1 hour"
    condition:
      field: action.action_type
      eq: "cart_reminder"
    action:
      type: schedule
      delay_seconds: 3600
"#;

/// Schedule a retry at off-peak time (simulated as 30-second delay).
const OFF_PEAK_RETRY_RULE: &str = r#"
rules:
  - name: schedule-off-peak-retry
    priority: 5
    description: "Schedule failed sync for off-peak retry in 30s"
    condition:
      field: action.action_type
      eq: "data_sync_retry"
    action:
      type: schedule
      delay_seconds: 30
"#;

/// Schedule an escalation 30 minutes (1800s) after an alert fires.
const ESCALATION_RULE: &str = r#"
rules:
  - name: schedule-escalation
    priority: 10
    description: "Schedule escalation 30 minutes after alert"
    condition:
      field: action.action_type
      eq: "escalation"
    action:
      type: schedule
      delay_seconds: 1800
"#;

/// Combined rule set: critical alerts fire immediately, non-critical are
/// scheduled for a 5-minute delay.
const COMBINED_ALERT_RULES: &str = r#"
rules:
  - name: critical-alert-immediate
    priority: 1
    description: "Critical alerts bypass scheduling and execute immediately"
    condition:
      all:
        - field: action.action_type
          eq: "alert"
        - field: action.payload.severity
          eq: "critical"
    action:
      type: reroute
      target_provider: pagerduty

  - name: schedule-non-critical-alert
    priority: 10
    description: "Non-critical alerts are scheduled for batch delivery"
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: schedule
      delay_seconds: 300
"#;

/// Short delay for batch processing window demonstration.
const BATCH_WINDOW_RULE: &str = r#"
rules:
  - name: schedule-report-generation
    priority: 10
    description: "Schedule report generation for batch window"
    condition:
      field: action.action_type
      eq: "generate_report"
    action:
      type: schedule
      delay_seconds: 60
"#;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       DELAYED / SCHEDULED ACTIONS SIMULATION DEMO            ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Delayed Email Reminder
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: DELAYED EMAIL REMINDER");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  When a user signs up, we schedule a welcome email for 24 hours");
    println!("  later. A cart-abandonment reminder is scheduled for 1 hour.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(WELCOME_EMAIL_RULE)
            .add_rule_yaml(CART_REMINDER_RULE)
            .build(),
    )
    .await?;

    println!("  Started simulation cluster");
    println!("  Registered 'email' recording provider");
    println!("  Loaded rules: schedule-welcome-email (24h), schedule-cart-reminder (1h)\n");

    // Sign-up triggers welcome email -- should be scheduled, not executed yet
    let welcome = Action::new(
        "onboarding",
        "saas-tenant",
        "email",
        "welcome_email",
        serde_json::json!({
            "to": "alice@example.com",
            "subject": "Welcome to Acteon!",
            "template": "welcome_v2",
        }),
    );

    println!("  [dispatch] Welcome email for alice@example.com");
    let outcome = harness.dispatch(&welcome).await?;
    print_scheduled_outcome(&outcome, "welcome_email");

    // Provider should NOT have been called yet -- action is scheduled
    println!(
        "  [verify]   Provider calls: {} (not yet executed -- scheduled for later)",
        harness.provider("email").unwrap().call_count()
    );
    assert_eq!(harness.provider("email").unwrap().call_count(), 0);

    // Cart abandonment -- also scheduled
    let cart = Action::new(
        "onboarding",
        "saas-tenant",
        "email",
        "cart_reminder",
        serde_json::json!({
            "to": "bob@example.com",
            "subject": "You left items in your cart",
            "cart_value": "$49.99",
        }),
    );

    println!("\n  [dispatch] Cart reminder for bob@example.com");
    let outcome = harness.dispatch(&cart).await?;
    print_scheduled_outcome(&outcome, "cart_reminder");

    println!(
        "  [verify]   Provider calls: {} (both emails deferred)",
        harness.provider("email").unwrap().call_count()
    );
    assert_eq!(harness.provider("email").unwrap().call_count(), 0);

    // Immediate email (action_type doesn't match schedule rules) goes through
    let receipt = Action::new(
        "onboarding",
        "saas-tenant",
        "email",
        "order_receipt",
        serde_json::json!({
            "to": "alice@example.com",
            "subject": "Your order confirmation",
            "order_id": "ORD-12345",
        }),
    );

    println!("\n  [dispatch] Order receipt for alice@example.com (no schedule rule)");
    let outcome = harness.dispatch(&receipt).await?;
    println!("  [result]   Outcome: {:?}", outcome_label(&outcome));
    println!(
        "  [verify]   Provider calls: {} (receipt sent immediately!)",
        harness.provider("email").unwrap().call_count()
    );
    assert_eq!(harness.provider("email").unwrap().call_count(), 1);

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // SCENARIO 2: Off-Peak Retry
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: OFF-PEAK RETRY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A data synchronization that failed during peak hours is");
    println!("  rescheduled for off-peak execution via a schedule rule.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("data-pipeline")
            .add_rule_yaml(OFF_PEAK_RETRY_RULE)
            .build(),
    )
    .await?;

    println!("  Started simulation cluster");
    println!("  Loaded rule: schedule-off-peak-retry (30s delay)\n");

    // The retry action matches the schedule rule
    let retry = Action::new(
        "data",
        "analytics-tenant",
        "data-pipeline",
        "data_sync_retry",
        serde_json::json!({
            "source": "warehouse-prod",
            "table": "events",
            "failed_at": "2026-02-07T14:30:00Z",
            "attempt": 2,
            "error": "connection timeout during peak load",
        }),
    );

    println!("  [dispatch] Data sync retry (attempt #2, failed during peak)");
    let outcome = harness.dispatch(&retry).await?;
    print_scheduled_outcome(&outcome, "data_sync_retry");

    println!(
        "  [verify]   Provider calls: {} (retry deferred to off-peak window)",
        harness.provider("data-pipeline").unwrap().call_count()
    );
    assert_eq!(harness.provider("data-pipeline").unwrap().call_count(), 0);

    // A different action type that doesn't match executes immediately
    let immediate_sync = Action::new(
        "data",
        "analytics-tenant",
        "data-pipeline",
        "data_sync",
        serde_json::json!({
            "source": "warehouse-prod",
            "table": "metrics",
        }),
    );

    println!("\n  [dispatch] Fresh data sync (not a retry -- executes immediately)");
    let outcome = harness.dispatch(&immediate_sync).await?;
    println!("  [result]   Outcome: {:?}", outcome_label(&outcome));
    println!(
        "  [verify]   Provider calls: {} (fresh sync ran immediately)",
        harness.provider("data-pipeline").unwrap().call_count()
    );
    assert_eq!(harness.provider("data-pipeline").unwrap().call_count(), 1);

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // SCENARIO 3: Escalation Workflow
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: ESCALATION WORKFLOW");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  An alert fires and an escalation is scheduled for 30 minutes.");
    println!("  Critical alerts bypass scheduling entirely and go to PagerDuty.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("pagerduty")
            .add_rule_yaml(COMBINED_ALERT_RULES)
            .add_rule_yaml(ESCALATION_RULE)
            .build(),
    )
    .await?;

    println!("  Started simulation cluster");
    println!("  Registered 'slack' and 'pagerduty' providers");
    println!("  Loaded rules:");
    println!("    - critical-alert-immediate (priority 1, reroute to pagerduty)");
    println!("    - schedule-non-critical-alert (priority 10, 5-minute delay)");
    println!("    - schedule-escalation (priority 10, 30-minute delay)\n");

    // Non-critical alert gets scheduled
    let warning_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "warning",
            "source": "api-gateway",
            "message": "Response latency above threshold (p99 > 500ms)",
        }),
    );

    println!("  [dispatch] WARNING alert from api-gateway");
    let outcome = harness.dispatch(&warning_alert).await?;
    print_scheduled_outcome(&outcome, "non-critical alert");

    // Critical alert bypasses scheduling
    let critical_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "critical",
            "source": "database",
            "message": "Primary database unreachable",
        }),
    );

    println!("\n  [dispatch] CRITICAL alert from database (bypasses scheduling)");
    let outcome = harness.dispatch(&critical_alert).await?;
    println!("  [result]   Outcome: {:?}", outcome_label(&outcome));
    println!(
        "  [verify]   PagerDuty calls: {} (critical alert escalated immediately!)",
        harness.provider("pagerduty").unwrap().call_count()
    );
    assert_eq!(harness.provider("pagerduty").unwrap().call_count(), 1);

    // Schedule an escalation for the warning alert
    let escalation = Action::new(
        "monitoring",
        "acme-corp",
        "pagerduty",
        "escalation",
        serde_json::json!({
            "original_alert": "api-gateway latency",
            "severity": "warning",
            "message": "Escalating: latency issue unresolved after initial alert",
            "escalation_level": 2,
        }),
    );

    println!("\n  [dispatch] Escalation for warning alert (scheduled for 30 min)");
    let outcome = harness.dispatch(&escalation).await?;
    print_scheduled_outcome(&outcome, "escalation");

    println!("\n  Final state:");
    println!(
        "    Slack calls: {} (warning alert was scheduled, not sent yet)",
        harness.provider("slack").unwrap().call_count()
    );
    println!(
        "    PagerDuty calls: {} (only the critical alert executed)",
        harness.provider("pagerduty").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // SCENARIO 4: Multi-Tenant Scheduling
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: MULTI-TENANT SCHEDULING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Two tenants schedule actions independently. Each tenant's");
    println!("  scheduled actions are isolated from the other.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(WELCOME_EMAIL_RULE)
            .build(),
    )
    .await?;

    println!("  Started simulation cluster");
    println!("  Loaded rule: schedule-welcome-email (24h delay)\n");

    // Tenant A schedules a welcome email
    let tenant_a_action = Action::new(
        "onboarding",
        "tenant-alpha",
        "email",
        "welcome_email",
        serde_json::json!({
            "to": "user-a@alpha.com",
            "subject": "Welcome to Alpha!",
        }),
    );

    println!("  [dispatch] Tenant ALPHA: welcome email for user-a@alpha.com");
    let outcome_a = harness.dispatch(&tenant_a_action).await?;
    print_scheduled_outcome(&outcome_a, "tenant-alpha");

    // Tenant B schedules a welcome email
    let beta_action = Action::new(
        "onboarding",
        "tenant-beta",
        "email",
        "welcome_email",
        serde_json::json!({
            "to": "user-b@beta.com",
            "subject": "Welcome to Beta!",
        }),
    );

    println!("\n  [dispatch] Tenant BETA: welcome email for user-b@beta.com");
    let outcome_b = harness.dispatch(&beta_action).await?;
    print_scheduled_outcome(&outcome_b, "tenant-beta");

    // Both are scheduled independently
    println!(
        "\n  [verify]   Provider calls: {} (both tenants' emails deferred)",
        harness.provider("email").unwrap().call_count()
    );
    assert_eq!(harness.provider("email").unwrap().call_count(), 0);

    // Verify that each tenant got a different action_id
    if let (
        ActionOutcome::Scheduled {
            action_id: id_a, ..
        },
        ActionOutcome::Scheduled {
            action_id: id_b, ..
        },
    ) = (&outcome_a, &outcome_b)
    {
        println!("  [verify]   Tenant ALPHA action_id: {id_a}");
        println!("  [verify]   Tenant BETA  action_id: {id_b}");
        assert_ne!(
            id_a, id_b,
            "each tenant should get a unique scheduled action ID"
        );
        println!("  [verify]   Unique action IDs confirmed (tenant isolation)");
    }

    // Tenant A sends a non-scheduled action -- only their immediate action runs
    let tenant_a_immediate = Action::new(
        "onboarding",
        "tenant-alpha",
        "email",
        "password_reset",
        serde_json::json!({
            "to": "user-a@alpha.com",
            "subject": "Reset your password",
        }),
    );

    println!("\n  [dispatch] Tenant ALPHA: password reset (immediate)");
    let outcome = harness.dispatch(&tenant_a_immediate).await?;
    println!("  [result]   Outcome: {:?}", outcome_label(&outcome));
    println!(
        "  [verify]   Provider calls: {} (only the immediate action executed)",
        harness.provider("email").unwrap().call_count()
    );
    assert_eq!(harness.provider("email").unwrap().call_count(), 1);

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // SCENARIO 5: Batch Processing Window
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: BATCH PROCESSING WINDOW");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Multiple small report-generation requests arrive and are all");
    println!("  scheduled for a batch processing window (60s delay). When the");
    println!("  window arrives, the background processor dispatches them all.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("report-engine")
            .add_rule_yaml(BATCH_WINDOW_RULE)
            .build(),
    )
    .await?;

    println!("  Started simulation cluster");
    println!("  Loaded rule: schedule-report-generation (60s batch window)\n");

    let report_requests = vec![
        ("acme-corp", "monthly-revenue", "January 2026"),
        ("acme-corp", "user-growth", "January 2026"),
        ("acme-corp", "churn-analysis", "Q4 2025"),
        ("acme-corp", "api-usage", "Week 5"),
        ("acme-corp", "cost-breakdown", "January 2026"),
    ];

    let mut scheduled_count = 0;
    for (tenant, report_name, period) in &report_requests {
        let action = Action::new(
            "analytics",
            *tenant,
            "report-engine",
            "generate_report",
            serde_json::json!({
                "report": report_name,
                "period": period,
                "format": "pdf",
            }),
        );

        println!("  [dispatch] Report: {report_name} ({period})");
        let outcome = harness.dispatch(&action).await?;

        if matches!(outcome, ActionOutcome::Scheduled { .. }) {
            scheduled_count += 1;
            println!("  [result]   Scheduled for batch window");
        } else {
            println!("  [result]   Unexpected: {:?}", outcome_label(&outcome));
        }
    }

    println!(
        "\n  [summary]  {} of {} reports scheduled for batch processing",
        scheduled_count,
        report_requests.len()
    );
    println!(
        "  [verify]   Provider calls: {} (nothing executed yet -- all batched)",
        harness.provider("report-engine").unwrap().call_count()
    );
    assert_eq!(scheduled_count, report_requests.len());
    assert_eq!(harness.provider("report-engine").unwrap().call_count(), 0);

    println!("\n  When the 60-second batch window elapses, the background");
    println!("  processor will dispatch all 5 reports to the report-engine");
    println!("  provider in a single batch window.");

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // SCENARIO 6: Dry-Run with Schedule Rules
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 6: DRY-RUN WITH SCHEDULE RULES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Dry-run mode shows what *would* happen without persisting any");
    println!("  scheduled action. Useful for testing rules before deployment.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(WELCOME_EMAIL_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "onboarding",
        "saas-tenant",
        "email",
        "welcome_email",
        serde_json::json!({
            "to": "test@example.com",
            "subject": "Welcome!",
        }),
    );

    println!("  [dry-run]  Welcome email for test@example.com");
    let outcome = harness.dispatch_dry_run(&action).await?;
    println!("  [result]   Outcome: {outcome:?}");

    match &outcome {
        ActionOutcome::DryRun {
            verdict,
            matched_rule,
            would_be_provider,
        } => {
            println!("  [detail]   Verdict: {verdict}");
            println!("  [detail]   Matched rule: {matched_rule:?}");
            println!("  [detail]   Would-be provider: {would_be_provider}");
        }
        _ => {
            println!(
                "  [detail]   (non-DryRun outcome: {:?})",
                outcome_label(&outcome)
            );
        }
    }

    println!(
        "  [verify]   Provider calls: {} (dry-run never executes or schedules)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n  Simulation shut down\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              SCHEDULED ACTIONS DEMO COMPLETE                 ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  Scenarios demonstrated:                                     ║");
    println!("║                                                              ║");
    println!("║  1. Delayed Email Reminder                                   ║");
    println!("║     - Welcome email scheduled for 24h after sign-up          ║");
    println!("║     - Cart reminder scheduled for 1h                         ║");
    println!("║     - Immediate emails bypass scheduling                     ║");
    println!("║                                                              ║");
    println!("║  2. Off-Peak Retry                                           ║");
    println!("║     - Failed sync retry deferred to quiet window             ║");
    println!("║     - Fresh syncs execute immediately                        ║");
    println!("║                                                              ║");
    println!("║  3. Escalation Workflow                                      ║");
    println!("║     - Critical alerts bypass scheduling (rerouted)           ║");
    println!("║     - Non-critical alerts scheduled for batch delivery       ║");
    println!("║     - Escalation scheduled for 30-minute follow-up           ║");
    println!("║                                                              ║");
    println!("║  4. Multi-Tenant Scheduling                                  ║");
    println!("║     - Independent scheduling per tenant                      ║");
    println!("║     - Unique action IDs per tenant (isolation)               ║");
    println!("║                                                              ║");
    println!("║  5. Batch Processing Window                                  ║");
    println!("║     - 5 reports scheduled for 60s batch window               ║");
    println!("║     - Zero provider calls until window elapses               ║");
    println!("║                                                              ║");
    println!("║  6. Dry-Run with Schedule Rules                              ║");
    println!("║     - Preview scheduling behavior without side effects       ║");
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

// =============================================================================
// Helper functions
// =============================================================================

/// Pretty-print a `Scheduled` outcome with context.
fn print_scheduled_outcome(outcome: &ActionOutcome, label: &str) {
    match outcome {
        ActionOutcome::Scheduled {
            action_id,
            scheduled_for,
        } => {
            println!("  [result]   Outcome: Scheduled");
            println!("  [detail]   Action ID: {action_id}");
            println!("  [detail]   Scheduled for: {scheduled_for}");
            println!("  [detail]   Label: {label}");
        }
        other => {
            println!(
                "  [result]   Unexpected outcome for {label}: {:?}",
                outcome_label(other)
            );
        }
    }
}

/// Return a short label for an outcome variant (for display purposes).
fn outcome_label(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "Executed",
        ActionOutcome::Deduplicated => "Deduplicated",
        ActionOutcome::Suppressed { .. } => "Suppressed",
        ActionOutcome::Rerouted { .. } => "Rerouted",
        ActionOutcome::Throttled { .. } => "Throttled",
        ActionOutcome::Failed(_) => "Failed",
        ActionOutcome::Grouped { .. } => "Grouped",
        ActionOutcome::StateChanged { .. } => "StateChanged",
        ActionOutcome::PendingApproval { .. } => "PendingApproval",
        ActionOutcome::ChainStarted { .. } => "ChainStarted",
        ActionOutcome::DryRun { .. } => "DryRun",
        ActionOutcome::CircuitOpen { .. } => "CircuitOpen",
        ActionOutcome::Scheduled { .. } => "Scheduled",
    }
}
