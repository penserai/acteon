//! Demonstration of the simulation framework in action.
//!
//! Run with: cargo run -p acteon-simulation --example demo_simulation

use acteon_core::Action;
use acteon_simulation::prelude::*;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           ACTEON SIMULATION FRAMEWORK DEMO                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Suppression Rules
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 1: SUPPRESSION RULES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster with 1 node");
    println!("✓ Registered 'email' recording provider");
    println!("✓ Loaded suppression rule: block-spam\n");

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

    println!("→ Dispatching SPAM action (action_type='spam')...");
    let outcome = harness.dispatch(&spam_action).await?;

    println!("  Action ID: {}", spam_action.id);
    println!("  Provider: {}", spam_action.provider);
    println!("  Action Type: {}", spam_action.action_type);
    println!("  Outcome: {:?}", outcome);
    println!(
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

    println!("→ Dispatching LEGITIMATE action (action_type='send_email')...");
    let outcome = harness.dispatch(&legit_action).await?;

    println!("  Action ID: {}", legit_action.id);
    println!("  Provider: {}", legit_action.provider);
    println!("  Action Type: {}", legit_action.action_type);
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Provider called: {} times",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Deduplication
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 2: DEDUPLICATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("push")
            .add_rule_yaml(DEDUP_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster");
    println!("✓ Loaded deduplication rule: dedup-notifications\n");

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

    println!("→ Dispatching FIRST notification (dedup_key='user-123-new-message')...");
    let outcome1 = harness.dispatch(&notify1).await?;
    println!("  Outcome: {:?}", outcome1);
    println!(
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

    println!("→ Dispatching DUPLICATE notification (same dedup_key)...");
    let outcome2 = harness.dispatch(&notify2).await?;
    println!("  Outcome: {:?}", outcome2);
    println!(
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

    println!("\n→ Dispatching DIFFERENT notification (dedup_key='user-123-order-shipped')...");
    let outcome3 = harness.dispatch(&notify3).await?;
    println!("  Outcome: {:?}", outcome3);
    println!(
        "  Provider called: {} times",
        harness.provider("push").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Rerouting
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 3: REROUTING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(REROUTE_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster");
    println!("✓ Registered 'email' and 'sms' providers");
    println!("✓ Loaded reroute rule: reroute-urgent\n");

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

    println!("→ Dispatching NORMAL priority action to 'email' provider...");
    let outcome = harness.dispatch(&normal).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Email provider called: {}",
        harness.provider("email").unwrap().call_count()
    );
    println!(
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

    println!("→ Dispatching URGENT priority action to 'email' provider...");
    let outcome = harness.dispatch(&urgent).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Email provider called: {} (still 1 - rerouted!)",
        harness.provider("email").unwrap().call_count()
    );
    println!(
        "  SMS provider called: {} (received the rerouted action!)",
        harness.provider("sms").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 4: Multi-Node Cluster with Shared State
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 4: MULTI-NODE CLUSTER WITH SHARED STATE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(3)
            .shared_state(true)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE.replace("notify", "send_email"))
            .build(),
    )
    .await?;

    println!("✓ Started 3-node cluster with SHARED state");
    println!("  Node 0: {}", harness.node(0).unwrap().base_url());
    println!("  Node 1: {}", harness.node(1).unwrap().base_url());
    println!("  Node 2: {}", harness.node(2).unwrap().base_url());
    println!();

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

    println!("→ Dispatching to NODE 0...");
    let outcome0 = harness.dispatch_to(0, &action).await?;
    println!("  Outcome: {:?}", outcome0);

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

    println!("\n→ Dispatching SAME dedup_key to NODE 1...");
    let outcome1 = harness.dispatch_to(1, &action).await?;
    println!("  Outcome: {:?} (deduplicated across nodes!)", outcome1);

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

    println!("\n→ Dispatching SAME dedup_key to NODE 2...");
    let outcome2 = harness.dispatch_to(2, &action).await?;
    println!("  Outcome: {:?}", outcome2);

    println!(
        "\n  Total provider calls: {} (only 1 despite 3 dispatches!)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 5: Batch Dispatch with Metrics
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 5: BATCH DISPATCH");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster\n");

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

    println!("→ Dispatching batch of 100 actions...");
    let start = std::time::Instant::now();
    let outcomes = harness.dispatch_batch(&actions).await;
    let elapsed = start.elapsed();

    let successful = outcomes.iter().filter(|r| r.is_ok()).count();
    let executed = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(acteon_core::ActionOutcome::Executed(_))))
        .count();

    println!("  Completed in: {:?}", elapsed);
    println!("  Successful: {}/100", successful);
    println!("  Executed: {}/100", executed);
    println!(
        "  Provider calls: {}",
        harness.provider("email").unwrap().call_count()
    );
    println!(
        "  Throughput: {:.0} actions/sec",
        100.0 / elapsed.as_secs_f64()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 6: PagerDuty Incident Escalation
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 6: PAGERDUTY INCIDENT ESCALATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("pagerduty")
            .add_rule_yaml(PAGERDUTY_ESCALATION_RULES)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster");
    println!("✓ Registered 'slack' and 'pagerduty' providers");
    println!("✓ Loaded escalation rules:");
    println!("  - critical severity → reroute to pagerduty");
    println!("  - high severity → deduplicate (10 min window)\n");

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

    println!("→ Dispatching LOW severity alert to 'slack'...");
    let outcome = harness.dispatch(&low_alert).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
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

    println!("→ Dispatching CRITICAL severity alert to 'slack'...");
    let outcome = harness.dispatch(&critical_alert).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
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

    println!("→ Dispatching FIRST high severity alert...");
    let outcome = harness.dispatch(&high_alert).await?;
    println!("  Outcome: {:?}", outcome);

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

    println!("→ Dispatching DUPLICATE high severity alert...");
    let outcome = harness.dispatch(&high_alert_dup).await?;
    println!(
        "  Outcome: {:?} (deduplicated - alert storm prevented!)",
        outcome
    );

    println!(
        "\n  Final counts: Slack={}, PagerDuty={}",
        harness.provider("slack").unwrap().call_count(),
        harness.provider("pagerduty").unwrap().call_count(),
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                    DEMO COMPLETE                             ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
