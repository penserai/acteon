//! Demonstration of the simulation framework in action.
//!
//! Run with: cargo run -p acteon-simulation --example demo_simulation

use acteon_core::Action;
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

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 1
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 300
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

const PAGERDUTY_ESCALATION_RULES: &str = r#"
rules:
  # Critical severity incidents get escalated to PagerDuty
  - name: escalate-critical-to-pagerduty
    priority: 1
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: pagerduty

  # High severity incidents get deduplicated (avoid alert storms)
  - name: dedup-high-severity
    priority: 5
    condition:
      field: action.payload.severity
      eq: "high"
    action:
      type: deduplicate
      ttl_seconds: 600
"#;

const WEBHOOK_REROUTE_RULES: &str = r#"
rules:
  # Reroute critical alerts from Slack to webhook for external integration
  - name: reroute-critical-to-webhook
    priority: 1
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: webhook
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║           ACTEON SIMULATION FRAMEWORK DEMO                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Suppression Rules
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: SUPPRESSION RULES");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'email' recording provider");
    info!("✓ Loaded suppression rule: block-spam\n");

    // Try to send spam - should be suppressed
    let spam_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Buy now!!!",
        }),
    );

    info!("→ Dispatching SPAM action (action_type='spam')...");
    let outcome = harness.dispatch(&spam_action).await?;

    info!("  Action ID: {}", spam_action.id);
    info!("  Provider: {}", spam_action.provider);
    info!("  Action Type: {}", spam_action.action_type);
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Provider called: {} times\n",
        harness.provider("email").unwrap().call_count()
    );

    // Try to send legitimate email - should execute
    let legit_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Your order has shipped",
        }),
    );

    info!("→ Dispatching LEGITIMATE action (action_type='send_email')...");
    let outcome = harness.dispatch(&legit_action).await?;

    info!("  Action ID: {}", legit_action.id);
    info!("  Provider: {}", legit_action.provider);
    info!("  Action Type: {}", legit_action.action_type);
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Provider called: {} times",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Deduplication
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: DEDUPLICATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("push")
            .add_rule_yaml(DEDUP_RULE)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster");
    info!("✓ Loaded deduplication rule: dedup-notifications\n");

    // Send first notification
    let notify1 = Action::new(
        "notifications",
        "tenant-1",
        "push",
        "notify",
        serde_json::json!({
            "user_id": "user-123",
            "message": "You have a new message",
        }),
    )
    .with_dedup_key("user-123-new-message");

    info!("→ Dispatching FIRST notification (dedup_key='user-123-new-message')...");
    let outcome1 = harness.dispatch(&notify1).await?;
    info!("  Outcome: {:?}", outcome1);
    info!(
        "  Provider called: {} times\n",
        harness.provider("push").unwrap().call_count()
    );

    // Send duplicate notification
    let notify2 = Action::new(
        "notifications",
        "tenant-1",
        "push",
        "notify",
        serde_json::json!({
            "user_id": "user-123",
            "message": "You have a new message",
        }),
    )
    .with_dedup_key("user-123-new-message");

    info!("→ Dispatching DUPLICATE notification (same dedup_key)...");
    let outcome2 = harness.dispatch(&notify2).await?;
    info!("  Outcome: {:?}", outcome2);
    info!(
        "  Provider called: {} times (still 1 - duplicate blocked!)",
        harness.provider("push").unwrap().call_count()
    );

    // Send different notification
    let notify3 = Action::new(
        "notifications",
        "tenant-1",
        "push",
        "notify",
        serde_json::json!({
            "user_id": "user-123",
            "message": "Your order shipped",
        }),
    )
    .with_dedup_key("user-123-order-shipped");

    info!("\n→ Dispatching DIFFERENT notification (dedup_key='user-123-order-shipped')...");
    let outcome3 = harness.dispatch(&notify3).await?;
    info!("  Outcome: {:?}", outcome3);
    info!(
        "  Provider called: {} times",
        harness.provider("push").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Rerouting
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: REROUTING");
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

    info!("✓ Started simulation cluster");
    info!("✓ Registered 'email' and 'sms' providers");
    info!("✓ Loaded reroute rule: reroute-urgent\n");

    // Send normal priority - should go to email
    let normal = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "alert",
        serde_json::json!({
            "priority": "normal",
            "message": "Monthly report ready",
        }),
    );

    info!("→ Dispatching NORMAL priority action to 'email' provider...");
    let outcome = harness.dispatch(&normal).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Email provider called: {}",
        harness.provider("email").unwrap().call_count()
    );
    info!(
        "  SMS provider called: {}\n",
        harness.provider("sms").unwrap().call_count()
    );

    // Send urgent priority - should reroute to SMS
    let urgent = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "alert",
        serde_json::json!({
            "priority": "urgent",
            "message": "Server down!",
        }),
    );

    info!("→ Dispatching URGENT priority action to 'email' provider...");
    let outcome = harness.dispatch(&urgent).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Email provider called: {} (still 1 - rerouted!)",
        harness.provider("email").unwrap().call_count()
    );
    info!(
        "  SMS provider called: {} (received the rerouted action!)",
        harness.provider("sms").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 4: Multi-Node Cluster with Shared State
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: MULTI-NODE CLUSTER WITH SHARED STATE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(3)
            .shared_state(true)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE.replace("notify", "send_email"))
            .build(),
    )
    .await?;

    info!("✓ Started 3-node cluster with SHARED state");
    info!("  Node 0: {}", harness.node(0).unwrap().base_url());
    info!("  Node 1: {}", harness.node(1).unwrap().base_url());
    info!("  Node 2: {}", harness.node(2).unwrap().base_url());
    info!("");

    // Send to node 0
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
        }),
    )
    .with_dedup_key("cross-node-dedup");

    info!("→ Dispatching to NODE 0...");
    let outcome0 = harness.dispatch_to(0, &action).await?;
    info!("  Outcome: {:?}", outcome0);

    // Send same dedup_key to node 1
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
        }),
    )
    .with_dedup_key("cross-node-dedup");

    info!("\n→ Dispatching SAME dedup_key to NODE 1...");
    let outcome1 = harness.dispatch_to(1, &action).await?;
    info!("  Outcome: {:?} (deduplicated across nodes!)", outcome1);

    // Send same dedup_key to node 2
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
        }),
    )
    .with_dedup_key("cross-node-dedup");

    info!("\n→ Dispatching SAME dedup_key to NODE 2...");
    let outcome2 = harness.dispatch_to(2, &action).await?;
    info!("  Outcome: {:?}", outcome2);

    info!(
        "\n  Total provider calls: {} (only 1 despite 3 dispatches!)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 5: Batch Dispatch with Metrics
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: BATCH DISPATCH");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster\n");

    let actions: Vec<Action> = (0..100)
        .map(|i| {
            Action::new(
                "bulk",
                "tenant-1",
                "email",
                "bulk_send",
                serde_json::json!({"recipient_id": i}),
            )
        })
        .collect();

    info!("→ Dispatching batch of 100 actions...");
    let start = std::time::Instant::now();
    let outcomes = harness.dispatch_batch(&actions).await;
    let elapsed = start.elapsed();

    let successful = outcomes.iter().filter(|r| r.is_ok()).count();
    let executed = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(acteon_core::ActionOutcome::Executed(_))))
        .count();

    info!("  Completed in: {:?}", elapsed);
    info!("  Successful: {}/100", successful);
    info!("  Executed: {}/100", executed);
    info!(
        "  Provider calls: {}",
        harness.provider("email").unwrap().call_count()
    );
    info!(
        "  Throughput: {:.0} actions/sec",
        100.0 / elapsed.as_secs_f64()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 6: PagerDuty Incident Escalation
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 6: PAGERDUTY INCIDENT ESCALATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("pagerduty")
            .add_rule_yaml(PAGERDUTY_ESCALATION_RULES)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster");
    info!("✓ Registered 'slack' and 'pagerduty' providers");
    info!("✓ Loaded escalation rules:");
    info!("  - critical severity → reroute to pagerduty");
    info!("  - high severity → deduplicate (10 min window)\n");

    // Low severity alert - goes to slack normally
    let low_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "low",
            "source": "health-check",
            "message": "Latency slightly above threshold",
        }),
    );

    info!("→ Dispatching LOW severity alert to 'slack'...");
    let outcome = harness.dispatch(&low_alert).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Slack called: {}, PagerDuty called: {}\n",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("pagerduty").unwrap().call_count(),
    );

    // Critical severity alert - rerouted to pagerduty
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

    info!("→ Dispatching CRITICAL severity alert to 'slack'...");
    let outcome = harness.dispatch(&critical_alert).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Slack called: {} (unchanged), PagerDuty called: {} (escalated!)\n",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("pagerduty").unwrap().call_count(),
    );

    // High severity alert - first one executes, duplicate is deduplicated
    let high_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "high",
            "source": "api-gateway",
            "message": "Error rate above 5%",
        }),
    )
    .with_dedup_key("api-gateway-error-rate");

    info!("→ Dispatching FIRST high severity alert...");
    let outcome = harness.dispatch(&high_alert).await?;
    info!("  Outcome: {:?}", outcome);

    let high_alert_dup = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "high",
            "source": "api-gateway",
            "message": "Error rate above 5%",
        }),
    )
    .with_dedup_key("api-gateway-error-rate");

    info!("→ Dispatching DUPLICATE high severity alert...");
    let outcome = harness.dispatch(&high_alert_dup).await?;
    info!(
        "  Outcome: {:?} (deduplicated - alert storm prevented!)",
        outcome
    );

    info!(
        "\n  Final counts: Slack={}, PagerDuty={}",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("pagerduty").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 7: Webhook Dispatch via Rerouting
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 7: WEBHOOK DISPATCH VIA REROUTING");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("webhook")
            .add_rule_yaml(WEBHOOK_REROUTE_RULES)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster");
    info!("✓ Registered 'slack' and 'webhook' providers");
    info!("✓ Loaded rule: reroute-critical-to-webhook\n");

    // Warning alert - stays on Slack
    let warning_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "warning",
            "source": "api-gateway",
            "message": "Response times elevated",
            "webhook_url": "https://hooks.example.com/alerts"
        }),
    );

    info!("→ Dispatching WARNING severity alert to 'slack'...");
    let outcome = harness.dispatch(&warning_alert).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Slack called: {}, Webhook called: {}\n",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("webhook").unwrap().call_count(),
    );

    // Critical alert - rerouted to webhook
    let critical_alert = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "critical",
            "source": "database",
            "message": "Primary database unreachable",
            "webhook_url": "https://hooks.example.com/incidents"
        }),
    );

    info!("→ Dispatching CRITICAL severity alert to 'slack'...");
    let outcome = harness.dispatch(&critical_alert).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Slack called: {} (unchanged), Webhook called: {} (rerouted!)",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("webhook").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║                    DEMO COMPLETE                             ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
