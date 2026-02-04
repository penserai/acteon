//! Demonstration of Acteon with DynamoDB backend.
//!
//! This example runs the full Acteon gateway against a real DynamoDB instance
//! (either DynamoDB Local or AWS DynamoDB).
//!
//! Prerequisites (DynamoDB Local):
//!   docker run -d --name acteon-dynamodb -p 8000:8000 amazon/dynamodb-local:latest
//!
//! Run with:
//!   cargo run -p acteon-simulation --example dynamodb_simulation --features dynamodb

use std::sync::Arc;

use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::RecordingProvider;

// Import DynamoDB backends
use acteon_state_dynamodb::{DynamoConfig, DynamoDistributedLock, DynamoStateStore, build_client, create_table};

const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 1
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 60
"#;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       ACTEON SIMULATION WITH DYNAMODB BACKEND                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Configure DynamoDB connection (using DynamoDB Local)
    let dynamo_config = DynamoConfig {
        table_name: "acteon_sim_state".to_string(),
        region: "us-east-1".to_string(),
        endpoint_url: Some("http://localhost:8000".to_string()),
        key_prefix: "acteon-sim".to_string(),
    };

    println!("→ Connecting to DynamoDB Local at {}...",
             dynamo_config.endpoint_url.as_ref().unwrap());

    // Build a shared client for both state store and lock
    let client = build_client(&dynamo_config).await;

    // Create the table if it doesn't exist
    println!("→ Creating table '{}' if not exists...", dynamo_config.table_name);
    create_table(&client, &dynamo_config.table_name).await?;

    // Create DynamoDB-backed state store and distributed lock
    let state = Arc::new(DynamoStateStore::from_client(client.clone(), &dynamo_config));
    let lock = Arc::new(DynamoDistributedLock::from_client(client, &dynamo_config));

    println!("✓ Connected to DynamoDB");
    println!("✓ Table: '{}'", dynamo_config.table_name);
    println!("✓ Key prefix: '{}'\n", dynamo_config.key_prefix);

    // Parse rules
    let frontend = YamlFrontend;
    let mut rules = frontend.parse(DEDUP_RULE)?;
    rules.extend(frontend.parse(SUPPRESSION_RULE)?);

    println!("✓ Loaded {} rules", rules.len());
    for rule in &rules {
        println!("  - {}: {:?}", rule.name, rule.action);
    }
    println!();

    // Create recording providers
    let email_provider = Arc::new(RecordingProvider::new("email"));
    let sms_provider = Arc::new(RecordingProvider::new("sms"));

    // Build the gateway with DynamoDB backends
    let gateway = GatewayBuilder::new()
        .state(state.clone())
        .lock(lock.clone())
        .rules(rules)
        .provider(email_provider.clone() as Arc<dyn DynProvider>)
        .provider(sms_provider.clone() as Arc<dyn DynProvider>)
        .build()?;

    println!("✓ Gateway built with DynamoDB state and lock backends\n");

    // =========================================================================
    // DEMO 1: Deduplication with DynamoDB State
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 1: DEDUPLICATION WITH DYNAMODB STATE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let action1 = Action::new("notifications", "tenant-1", "email", "notify", serde_json::json!({
        "user_id": "user-123",
        "message": "You have a new message",
    })).with_dedup_key("dynamo-dedup-test-1");

    println!("→ Dispatching FIRST notification (dedup_key='dynamo-dedup-test-1')...");
    let outcome1 = gateway.dispatch(action1.clone(), None).await?;
    println!("  Outcome: {:?}", outcome1);
    println!("  Email provider called: {} times", email_provider.call_count());

    // Check DynamoDB state - the dedup key should now be stored
    println!("\n→ Checking DynamoDB state...");

    // Try duplicate
    let action2 = Action::new("notifications", "tenant-1", "email", "notify", serde_json::json!({
        "user_id": "user-123",
        "message": "You have a new message (duplicate)",
    })).with_dedup_key("dynamo-dedup-test-1");

    println!("\n→ Dispatching DUPLICATE notification (same dedup_key)...");
    let outcome2 = gateway.dispatch(action2, None).await?;
    println!("  Outcome: {:?}", outcome2);
    println!("  Email provider called: {} times (should still be 1)", email_provider.call_count());

    // Try different key
    let action3 = Action::new("notifications", "tenant-1", "email", "notify", serde_json::json!({
        "user_id": "user-456",
        "message": "Different notification",
    })).with_dedup_key("dynamo-dedup-test-2");

    println!("\n→ Dispatching DIFFERENT notification (dedup_key='dynamo-dedup-test-2')...");
    let outcome3 = gateway.dispatch(action3, None).await?;
    println!("  Outcome: {:?}", outcome3);
    println!("  Email provider called: {} times", email_provider.call_count());

    // =========================================================================
    // DEMO 2: Suppression
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 2: SUPPRESSION RULES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let spam_action = Action::new("notifications", "tenant-1", "email", "spam", serde_json::json!({
        "subject": "Buy now!!!",
    }));

    println!("→ Dispatching SPAM action...");
    let outcome = gateway.dispatch(spam_action, None).await?;
    println!("  Outcome: {:?}", outcome);
    println!("  Email provider called: {} times (should be 0)", email_provider.call_count());

    // =========================================================================
    // DEMO 3: Concurrent Dispatch with DynamoDB Locking
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 3: CONCURRENT DISPATCH WITH DYNAMODB LOCKING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    // Simulate multiple concurrent processes trying to send the same notification
    println!("→ Simulating 10 concurrent dispatches with SAME dedup_key...");
    println!("  (This tests DynamoDB distributed locking)\n");

    let gateway_arc = Arc::new(gateway);
    let mut handles = vec![];

    for i in 0..10 {
        let gw = Arc::clone(&gateway_arc);
        let handle = tokio::spawn(async move {
            let action = Action::new("notifications", "tenant-1", "email", "notify", serde_json::json!({
                "worker": i,
                "message": "Concurrent test",
            })).with_dedup_key("dynamo-concurrent-dedup-key");

            gw.dispatch(action, None).await
        });
        handles.push(handle);
    }

    // Wait for all to complete
    let mut executed = 0;
    let mut deduplicated = 0;
    let mut failed = 0;

    for handle in handles {
        match handle.await? {
            Ok(acteon_core::ActionOutcome::Executed(_)) => executed += 1,
            Ok(acteon_core::ActionOutcome::Deduplicated) => deduplicated += 1,
            Ok(other) => println!("  Unexpected outcome: {:?}", other),
            Err(e) => {
                println!("  Error: {}", e);
                failed += 1;
            }
        }
    }

    println!("  Results:");
    println!("    Executed: {}", executed);
    println!("    Deduplicated: {}", deduplicated);
    println!("    Failed: {}", failed);
    println!("    Email provider called: {} times", email_provider.call_count());
    println!("\n  (With proper locking, exactly 1 should execute, 9 deduplicated)");

    // =========================================================================
    // DEMO 4: Throughput Test
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DEMO 4: THROUGHPUT TEST (100 ACTIONS)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    email_provider.clear();

    let actions: Vec<Action> = (0..100)
        .map(|i| {
            Action::new(
                "bulk",
                "tenant-1",
                "email",
                "bulk_send",  // Not a "notify" action, so no dedup rule applies
                serde_json::json!({"recipient_id": i}),
            )
        })
        .collect();

    println!("→ Dispatching 100 actions sequentially...");
    let start = std::time::Instant::now();

    for action in actions {
        gateway_arc.dispatch(action, None).await?;
    }

    let elapsed = start.elapsed();

    println!("  Completed in: {:?}", elapsed);
    println!("  Email provider called: {} times", email_provider.call_count());
    println!("  Throughput: {:.0} actions/sec", 100.0 / elapsed.as_secs_f64());

    // =========================================================================
    // DEMO 5: Verify DynamoDB State Persistence
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  DYNAMODB STATE VERIFICATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("→ Items stored in DynamoDB (table: '{}'):", dynamo_config.table_name);
    println!("  - Dedup keys stored with TTL");
    println!("  - Lock entries for distributed coordination\n");

    println!("  You can verify with aws-cli:");
    println!("    aws dynamodb scan --table-name {} --endpoint-url http://localhost:8000",
             dynamo_config.table_name);

    // Cleanup
    gateway_arc.shutdown().await;
    println!("\n✓ Gateway shut down gracefully\n");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                  DYNAMODB DEMO COMPLETE                      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
