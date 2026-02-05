//! Demonstration of event grouping and state machine scenarios.
//!
//! This example shows various scenarios with event grouping, state machines,
//! and mixed configurations including scenarios without grouping for comparison.
//!
//! Run with: cargo run -p acteon-simulation --example grouping_simulation

use acteon_core::Action;
use acteon_simulation::prelude::*;

// =============================================================================
// Grouping Rules
// =============================================================================

/// Groups alerts by cluster and severity, waiting 30s before sending
const ALERT_GROUPING_RULE: &str = r#"
rules:
  - name: group-alerts-by-cluster
    priority: 10
    condition:
      field: action.action_type
      eq: alert
    action:
      type: group
      group_by:
        - tenant
        - payload.cluster
        - payload.severity
      group_wait_seconds: 30
      group_interval_seconds: 300
      max_group_size: 100
"#;

/// Groups notifications by user to batch multiple updates
const NOTIFICATION_BATCHING_RULE: &str = r#"
rules:
  - name: batch-user-notifications
    priority: 10
    condition:
      field: action.action_type
      eq: notification
    action:
      type: group
      group_by:
        - tenant
        - payload.user_id
      group_wait_seconds: 60
      group_interval_seconds: 600
      max_group_size: 50
"#;

/// State machine for ticket lifecycle
const TICKET_STATE_MACHINE_RULE: &str = r#"
rules:
  - name: ticket-lifecycle
    priority: 5
    condition:
      field: action.action_type
      eq: ticket
    action:
      type: state_machine
      state_machine: ticket
      fingerprint_fields:
        - action_type
        - payload.ticket_id
"#;

// =============================================================================
// Non-Grouping Rules (for comparison)
// =============================================================================

/// Simple suppression - no grouping
const SPAM_SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: spam
    action:
      type: suppress
"#;

/// Simple deduplication - no grouping
const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-emails
    priority: 5
    condition:
      field: action.action_type
      eq: email
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

/// High-priority rerouting - no grouping
const URGENT_REROUTE_RULE: &str = r#"
rules:
  - name: reroute-urgent
    priority: 1
    condition:
      field: action.payload.priority
      eq: urgent
    action:
      type: reroute
      target_provider: sms
"#;

// =============================================================================
// Combined Rules (Grouping + Other)
// =============================================================================

/// Combines grouping with priority-based rerouting
const COMBINED_RULES: &str = r#"
rules:
  # Urgent alerts bypass grouping and go straight to SMS
  - name: urgent-bypass
    priority: 1
    condition:
      all:
        - field: action.action_type
          eq: alert
        - field: action.payload.severity
          eq: critical
    action:
      type: reroute
      target_provider: sms

  # Non-urgent alerts get grouped
  - name: group-non-urgent-alerts
    priority: 10
    condition:
      field: action.action_type
      eq: alert
    action:
      type: group
      group_by:
        - tenant
        - payload.cluster
      group_wait_seconds: 60
      group_interval_seconds: 300
      max_group_size: 50

  # Suppress known noisy alerts
  - name: suppress-noisy
    priority: 2
    condition:
      all:
        - field: action.action_type
          eq: alert
        - field: action.payload.source
          eq: noisy-service
    action:
      type: suppress
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       EVENT GROUPING & STATE MACHINE SIMULATION DEMO         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Basic Alert Grouping
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: BASIC ALERT GROUPING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_rule_yaml(ALERT_GROUPING_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation with alert grouping rule");
    println!("  Grouping by: tenant, cluster, severity");
    println!("  Wait time: 30s, Max size: 100\n");

    // Send multiple alerts for the same cluster
    let alerts = vec![
        ("pod-crash", "prod-cluster", "warning"),
        ("memory-high", "prod-cluster", "warning"),
        ("cpu-spike", "prod-cluster", "warning"),
        ("disk-full", "prod-cluster", "critical"),
    ];

    for (source, cluster, severity) in &alerts {
        let action = Action::new(
            "monitoring",
            "acme-corp",
            "slack",
            "alert",
            serde_json::json!({
                "source": source,
                "cluster": cluster,
                "severity": severity,
                "message": format!("{} alert from {}", severity, source),
            }),
        );

        println!("→ Dispatching alert: {} ({}/{})", source, cluster, severity);
        let outcome = harness.dispatch(&action).await?;
        println!("  Outcome: {:?}", outcome);
    }

    println!(
        "\n  Provider calls: {} (alerts are being grouped!)",
        harness.provider("slack").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 2: No Grouping - Direct Execution
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: NO GROUPING - DIRECT EXECUTION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(), // No rules - direct execution
    )
    .await?;

    println!("✓ Started simulation WITHOUT grouping rules\n");

    // Send multiple actions - each executes immediately
    for i in 1..=5 {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "order_update",
            serde_json::json!({
                "order_id": format!("ORD-{}", i),
                "status": "shipped",
            }),
        );

        println!("→ Dispatching order update #{}", i);
        let outcome = harness.dispatch(&action).await?;
        println!("  Outcome: {:?}", outcome);
    }

    println!(
        "\n  Provider calls: {} (each action executed immediately!)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 3: Suppression vs Grouping Comparison
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: SUPPRESSION vs GROUPING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SPAM_SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started with SUPPRESSION rule (no grouping)\n");

    // Spam is completely blocked
    let spam = Action::new(
        "ns",
        "tenant",
        "email",
        "spam",
        serde_json::json!({"content": "Buy now!!!"}),
    );

    println!("→ Dispatching spam action...");
    let outcome = harness.dispatch(&spam).await?;
    println!("  Outcome: {:?}", outcome);
    println!("  (Suppressed = permanently blocked, not grouped for later)\n");

    // Legitimate email goes through
    let legit = Action::new(
        "ns",
        "tenant",
        "email",
        "welcome",
        serde_json::json!({"to": "user@example.com"}),
    );

    println!("→ Dispatching legitimate email...");
    let outcome = harness.dispatch(&legit).await?;
    println!("  Outcome: {:?}", outcome);

    println!(
        "\n  Provider calls: {} (only legitimate email executed)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 4: Notification Batching by User
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: NOTIFICATION BATCHING BY USER");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("push")
            .add_rule_yaml(NOTIFICATION_BATCHING_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started with notification batching rule");
    println!("  Grouping by: tenant, user_id");
    println!("  Wait time: 60s, Max size: 50\n");

    // Multiple notifications for the same user get batched
    let user_notifications = vec![
        ("user-123", "New comment on your post"),
        ("user-123", "Someone liked your photo"),
        ("user-123", "New follower request"),
        ("user-456", "Your order shipped"),
        ("user-456", "Delivery update"),
    ];

    for (user_id, message) in &user_notifications {
        let action = Action::new(
            "social",
            "app-tenant",
            "push",
            "notification",
            serde_json::json!({
                "user_id": user_id,
                "message": message,
            }),
        );

        println!("→ Notification for {}: \"{}\"", user_id, message);
        let outcome = harness.dispatch(&action).await?;
        println!("  Outcome: {:?}", outcome);
    }

    println!(
        "\n  Provider calls: {} (notifications batched by user!)",
        harness.provider("push").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 5: Deduplication (Without Grouping)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: DEDUPLICATION (NO GROUPING)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started with DEDUPLICATION rule (not grouping)");
    println!("  Dedup TTL: 300 seconds\n");

    // First email executes
    let email1 = Action::new(
        "ns",
        "tenant",
        "email",
        "email",
        serde_json::json!({"to": "user@example.com"}),
    )
    .with_dedup_key("welcome-user-123");

    println!("→ First email (dedup_key='welcome-user-123')");
    let outcome = harness.dispatch(&email1).await?;
    println!("  Outcome: {:?}", outcome);

    // Duplicate is blocked completely (not grouped)
    let email2 = Action::new(
        "ns",
        "tenant",
        "email",
        "email",
        serde_json::json!({"to": "user@example.com"}),
    )
    .with_dedup_key("welcome-user-123");

    println!("\n→ Duplicate email (same dedup_key)");
    let outcome = harness.dispatch(&email2).await?;
    println!("  Outcome: {:?}", outcome);
    println!("  (Deduplicated = blocked, NOT grouped for later batch)\n");

    // Different key executes
    let email3 = Action::new(
        "ns",
        "tenant",
        "email",
        "email",
        serde_json::json!({"to": "user@example.com"}),
    )
    .with_dedup_key("password-reset-user-123");

    println!("→ Different email (dedup_key='password-reset-user-123')");
    let outcome = harness.dispatch(&email3).await?;
    println!("  Outcome: {:?}", outcome);

    println!(
        "\n  Provider calls: {} (dedup blocked duplicate, others executed)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 6: Combined Rules - Critical Bypass + Grouping
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 6: CRITICAL BYPASS + GROUPING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("slack")
            .add_recording_provider("sms")
            .add_rule_yaml(COMBINED_RULES)
            .build(),
    )
    .await?;

    println!("✓ Started with COMBINED rules:");
    println!("  - Critical alerts → bypass to SMS (immediate)");
    println!("  - Non-critical alerts → grouped for batching");
    println!("  - Noisy source → suppressed entirely\n");

    // Critical alert bypasses grouping
    let critical = Action::new(
        "monitoring",
        "acme",
        "slack",
        "alert",
        serde_json::json!({
            "cluster": "prod",
            "severity": "critical",
            "message": "Database down!",
        }),
    );

    println!("→ CRITICAL alert (should bypass grouping, go to SMS)");
    let outcome = harness.dispatch(&critical).await?;
    println!("  Outcome: {:?}", outcome);

    // Warning alert gets grouped
    let warning = Action::new(
        "monitoring",
        "acme",
        "slack",
        "alert",
        serde_json::json!({
            "cluster": "prod",
            "severity": "warning",
            "message": "High memory usage",
        }),
    );

    println!("\n→ WARNING alert (should be grouped)");
    let outcome = harness.dispatch(&warning).await?;
    println!("  Outcome: {:?}", outcome);

    // Noisy alert gets suppressed
    let noisy = Action::new(
        "monitoring",
        "acme",
        "slack",
        "alert",
        serde_json::json!({
            "cluster": "prod",
            "severity": "warning",
            "source": "noisy-service",
            "message": "Routine noise",
        }),
    );

    println!("\n→ NOISY alert (should be suppressed)");
    let outcome = harness.dispatch(&noisy).await?;
    println!("  Outcome: {:?}", outcome);

    println!(
        "\n  Slack provider calls: {}",
        harness.provider("slack").unwrap().call_count()
    );
    println!(
        "  SMS provider calls: {} (critical alert bypassed grouping!)",
        harness.provider("sms").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 7: Multi-Node Group Coordination
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 7: MULTI-NODE GROUP COORDINATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(3)
            .shared_state(true) // Critical for group coordination
            .add_recording_provider("slack")
            .add_rule_yaml(ALERT_GROUPING_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started 3-node cluster with SHARED state");
    println!("  Node 0: {}", harness.node(0).unwrap().base_url());
    println!("  Node 1: {}", harness.node(1).unwrap().base_url());
    println!("  Node 2: {}", harness.node(2).unwrap().base_url());
    println!("  Grouping by: tenant, cluster, severity\n");

    // Same alert type sent to different nodes - should be in same group
    for (node_idx, source) in [(0, "service-a"), (1, "service-b"), (2, "service-c")] {
        let action = Action::new(
            "monitoring",
            "acme-corp",
            "slack",
            "alert",
            serde_json::json!({
                "source": source,
                "cluster": "prod",
                "severity": "warning",
                "message": format!("Alert from {}", source),
            }),
        );

        println!("→ Dispatching to NODE {} (source: {})", node_idx, source);
        let outcome = harness.dispatch_to(node_idx, &action).await?;
        println!("  Outcome: {:?}", outcome);
    }

    println!(
        "\n  Total provider calls: {} (all alerts grouped despite multi-node!)",
        harness.provider("slack").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 8: Urgent Rerouting (No Grouping)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 8: URGENT REROUTING (NO GROUPING)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml(URGENT_REROUTE_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started with REROUTING rule (no grouping)\n");

    // Normal priority - goes to original provider
    let normal = Action::new(
        "notifications",
        "tenant",
        "email",
        "alert",
        serde_json::json!({
            "priority": "normal",
            "message": "Weekly report ready",
        }),
    );

    println!("→ Normal priority action (target: email)");
    let outcome = harness.dispatch(&normal).await?;
    println!("  Outcome: {:?}", outcome);

    // Urgent priority - rerouted immediately (not grouped)
    let urgent = Action::new(
        "notifications",
        "tenant",
        "email",
        "alert",
        serde_json::json!({
            "priority": "urgent",
            "message": "Server down!",
        }),
    );

    println!("\n→ Urgent priority action (should reroute to SMS immediately)");
    let outcome = harness.dispatch(&urgent).await?;
    println!("  Outcome: {:?}", outcome);

    println!(
        "\n  Email provider calls: {}",
        harness.provider("email").unwrap().call_count()
    );
    println!(
        "  SMS provider calls: {} (urgent rerouted immediately, no grouping)",
        harness.provider("sms").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // SCENARIO 9: High Volume Batch
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 9: HIGH VOLUME BATCH (GROUPED vs NON-GROUPED)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // First: without grouping
    let harness_no_group = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("push")
            .build(),
    )
    .await?;

    println!("✓ Test A: 100 notifications WITHOUT grouping\n");

    let actions: Vec<Action> = (0..100)
        .map(|i| {
            Action::new(
                "social",
                "app",
                "push",
                "notification",
                serde_json::json!({
                    "user_id": format!("user-{}", i % 10), // 10 different users
                    "message": format!("Notification #{}", i),
                }),
            )
        })
        .collect();

    let start = std::time::Instant::now();
    let outcomes = harness_no_group.dispatch_batch(&actions).await;
    let elapsed_no_group = start.elapsed();

    let executed = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(acteon_core::ActionOutcome::Executed(_))))
        .count();

    println!("  Dispatched: 100 actions");
    println!("  Executed: {}", executed);
    println!(
        "  Provider calls: {}",
        harness_no_group.provider("push").unwrap().call_count()
    );
    println!("  Time: {:?}", elapsed_no_group);

    harness_no_group.teardown().await?;

    // Second: with grouping
    let harness_group = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("push")
            .add_rule_yaml(NOTIFICATION_BATCHING_RULE)
            .build(),
    )
    .await?;

    println!("\n✓ Test B: 100 notifications WITH grouping (by user)\n");

    let actions: Vec<Action> = (0..100)
        .map(|i| {
            Action::new(
                "social",
                "app",
                "push",
                "notification",
                serde_json::json!({
                    "user_id": format!("user-{}", i % 10), // 10 different users
                    "message": format!("Notification #{}", i),
                }),
            )
        })
        .collect();

    let start = std::time::Instant::now();
    let outcomes = harness_group.dispatch_batch(&actions).await;
    let elapsed_group = start.elapsed();

    let grouped = outcomes
        .iter()
        .filter(|r| matches!(r, Ok(acteon_core::ActionOutcome::Grouped { .. })))
        .count();

    println!("  Dispatched: 100 actions");
    println!("  Grouped: {} (into ~10 groups by user)", grouped);
    println!(
        "  Provider calls: {} (batched!)",
        harness_group.provider("push").unwrap().call_count()
    );
    println!("  Time: {:?}", elapsed_group);

    harness_group.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                    SIMULATION COMPLETE                       ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  Scenarios demonstrated:                                     ║");
    println!("║                                                              ║");
    println!("║  WITH GROUPING:                                              ║");
    println!("║    1. Basic alert grouping by cluster/severity               ║");
    println!("║    4. Notification batching by user                          ║");
    println!("║    6. Critical bypass + grouping combined                    ║");
    println!("║    7. Multi-node group coordination                          ║");
    println!("║    9. High volume batch comparison                           ║");
    println!("║                                                              ║");
    println!("║  WITHOUT GROUPING:                                           ║");
    println!("║    2. Direct execution (no rules)                            ║");
    println!("║    3. Suppression (blocks completely)                        ║");
    println!("║    5. Deduplication (blocks duplicates)                      ║");
    println!("║    8. Urgent rerouting (immediate delivery)                  ║");
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
