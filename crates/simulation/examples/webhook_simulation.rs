//! Webhook provider simulation scenarios.
//!
//! Demonstrates dispatching actions to the webhook provider,
//! rerouting to webhooks, and deduplication of webhook calls.
//!
//! Run with: cargo run -p acteon-simulation --example webhook_simulation

use acteon_core::Action;
use acteon_simulation::prelude::*;

const REROUTE_TO_WEBHOOK_RULE: &str = r#"
rules:
  - name: reroute-to-webhook
    priority: 1
    condition:
      field: action.payload.delivery_method
      eq: "webhook"
    action:
      type: reroute
      target_provider: webhook
"#;

const DEDUP_WEBHOOK_RULE: &str = r#"
rules:
  - name: dedup-webhook
    priority: 1
    condition:
      field: action.action_type
      eq: "webhook_notify"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          WEBHOOK PROVIDER SIMULATION DEMO                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Basic Webhook Dispatch
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 1: BASIC WEBHOOK DISPATCH");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster with 1 node");
    println!("✓ Registered 'webhook' recording provider\n");

    let webhook_action = Action::new(
        "integrations",
        "tenant-1",
        "webhook",
        "webhook_notify",
        serde_json::json!({
            "url": "https://hooks.example.com/alert",
            "method": "POST",
            "body": {
                "message": "Deployment completed",
                "environment": "production",
                "status": "success"
            },
            "headers": {
                "X-Source": "acteon"
            }
        }),
    );

    println!("→ Dispatching webhook action to https://hooks.example.com/alert...");
    let outcome = harness.dispatch(&webhook_action).await?;

    println!("  Action ID: {}", webhook_action.id);
    println!("  Provider: {}", webhook_action.provider);
    println!("  Action Type: {}", webhook_action.action_type);
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Webhook provider called: {} times\n",
        harness.provider("webhook").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Rerouting to Webhook
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 2: REROUTING TO WEBHOOK");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("webhook")
            .add_rule_yaml(REROUTE_TO_WEBHOOK_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster");
    println!("✓ Registered 'email' and 'webhook' providers");
    println!("✓ Loaded rule: reroute-to-webhook\n");

    // Normal email - goes to email provider
    let email_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Monthly report",
            "delivery_method": "email"
        }),
    );

    println!("→ Dispatching action with delivery_method='email' to 'email' provider...");
    let outcome = harness.dispatch(&email_action).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Email called: {}, Webhook called: {}\n",
        harness.provider("email").unwrap().call_count(),
        harness.provider("webhook").unwrap().call_count(),
    );

    // Webhook delivery - rerouted from email to webhook
    let webhook_delivery = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Critical alert",
            "delivery_method": "webhook",
            "webhook_url": "https://hooks.slack.com/services/xxx"
        }),
    );

    println!("→ Dispatching action with delivery_method='webhook' to 'email' provider...");
    let outcome = harness.dispatch(&webhook_delivery).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Email called: {} (unchanged), Webhook called: {} (rerouted!)\n",
        harness.provider("email").unwrap().call_count(),
        harness.provider("webhook").unwrap().call_count(),
    );

    harness.teardown().await?;
    println!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Webhook with Deduplication
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 3: WEBHOOK WITH DEDUPLICATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(DEDUP_WEBHOOK_RULE)
            .build(),
    )
    .await?;

    println!("✓ Started simulation cluster");
    println!("✓ Loaded deduplication rule: dedup-webhook\n");

    let first_webhook = Action::new(
        "monitoring",
        "tenant-1",
        "webhook",
        "webhook_notify",
        serde_json::json!({
            "url": "https://hooks.example.com/incidents",
            "body": {
                "incident": "high-cpu",
                "host": "web-01"
            }
        }),
    )
    .with_dedup_key("incident-web-01-high-cpu");

    println!("→ Dispatching FIRST webhook (dedup_key='incident-web-01-high-cpu')...");
    let outcome = harness.dispatch(&first_webhook).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Webhook called: {} times\n",
        harness.provider("webhook").unwrap().call_count()
    );

    let duplicate_webhook = Action::new(
        "monitoring",
        "tenant-1",
        "webhook",
        "webhook_notify",
        serde_json::json!({
            "url": "https://hooks.example.com/incidents",
            "body": {
                "incident": "high-cpu",
                "host": "web-01"
            }
        }),
    )
    .with_dedup_key("incident-web-01-high-cpu");

    println!("→ Dispatching DUPLICATE webhook (same dedup_key)...");
    let outcome = harness.dispatch(&duplicate_webhook).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Webhook called: {} times (still 1 - duplicate blocked!)\n",
        harness.provider("webhook").unwrap().call_count()
    );

    // Different dedup key - should execute
    let different_webhook = Action::new(
        "monitoring",
        "tenant-1",
        "webhook",
        "webhook_notify",
        serde_json::json!({
            "url": "https://hooks.example.com/incidents",
            "body": {
                "incident": "disk-full",
                "host": "db-01"
            }
        }),
    )
    .with_dedup_key("incident-db-01-disk-full");

    println!("→ Dispatching DIFFERENT webhook (dedup_key='incident-db-01-disk-full')...");
    let outcome = harness.dispatch(&different_webhook).await?;
    println!("  Outcome: {:?}", outcome);
    println!(
        "  Webhook called: {} times",
        harness.provider("webhook").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n✓ Simulation cluster shut down\n");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              WEBHOOK SIMULATION COMPLETE                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
