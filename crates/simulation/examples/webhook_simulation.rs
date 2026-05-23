//! Webhook provider simulation scenarios.
//!
//! Demonstrates dispatching actions to the webhook provider,
//! rerouting to webhooks, and deduplication of webhook calls.
//!
//! Run with: cargo run -p acteon-simulation --example webhook_simulation

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

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
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          WEBHOOK PROVIDER SIMULATION DEMO                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // DEMO 1: Basic Webhook Dispatch
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: BASIC WEBHOOK DISPATCH");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster with 1 node");
    info!("✓ Registered 'webhook' recording provider\n");

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

    info!("→ Dispatching webhook action to https://hooks.example.com/alert...");
    let outcome = harness.dispatch(&webhook_action).await?;

    info!("  Action ID: {}", webhook_action.id);
    info!("  Provider: {}", webhook_action.provider);
    info!("  Action Type: {}", webhook_action.action_type);
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Webhook provider called: {} times\n",
        harness.provider("webhook").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 2: Rerouting to Webhook
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: REROUTING TO WEBHOOK");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("webhook")
            .add_rule_yaml(REROUTE_TO_WEBHOOK_RULE)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster");
    info!("✓ Registered 'email' and 'webhook' providers");
    info!("✓ Loaded rule: reroute-to-webhook\n");

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

    info!("→ Dispatching action with delivery_method='email' to 'email' provider...");
    let outcome = harness.dispatch(&email_action).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
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

    info!("→ Dispatching action with delivery_method='webhook' to 'email' provider...");
    let outcome = harness.dispatch(&webhook_delivery).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Email called: {} (unchanged), Webhook called: {} (rerouted!)\n",
        harness.provider("email").unwrap().call_count(),
        harness.provider("webhook").unwrap().call_count(),
    );

    harness.teardown().await?;
    info!("✓ Simulation cluster shut down\n");

    // =========================================================================
    // DEMO 3: Webhook with Deduplication
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: WEBHOOK WITH DEDUPLICATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(DEDUP_WEBHOOK_RULE)
            .build(),
    )
    .await?;

    info!("✓ Started simulation cluster");
    info!("✓ Loaded deduplication rule: dedup-webhook\n");

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

    info!("→ Dispatching FIRST webhook (dedup_key='incident-web-01-high-cpu')...");
    let outcome = harness.dispatch(&first_webhook).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
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

    info!("→ Dispatching DUPLICATE webhook (same dedup_key)...");
    let outcome = harness.dispatch(&duplicate_webhook).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
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

    info!("→ Dispatching DIFFERENT webhook (dedup_key='incident-db-01-disk-full')...");
    let outcome = harness.dispatch(&different_webhook).await?;
    info!("  Outcome: {:?}", outcome);
    info!(
        "  Webhook called: {} times",
        harness.provider("webhook").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n✓ Simulation cluster shut down\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              WEBHOOK SIMULATION COMPLETE                    ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
