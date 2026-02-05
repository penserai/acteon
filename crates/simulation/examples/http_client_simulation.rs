//! End-to-end HTTP simulation against a running Acteon server.
//!
//! This example uses a generic HTTP client to dispatch actions via the REST API,
//! providing realistic integration testing.
//!
//! Prerequisites:
//!   # Start the server with rules
//!   cargo run -p acteon-server -- -c examples/redis.toml
//!
//!   # Or with PostgreSQL for audit
//!   docker compose --profile postgres up -d
//!   cargo run -p acteon-server -- -c examples/postgres.toml
//!
//! Then run this simulation:
//!   cargo run -p acteon-simulation --example http_client_simulation
//!
//! Environment variables:
//!   ACTEON_URL - Server URL (default: http://localhost:8080)

use acteon_core::Action;
use acteon_simulation::{ActeonClient, AuditQuery};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║     HTTP CLIENT SIMULATION (End-to-End via REST API)         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Create HTTP client
    let base_url =
        std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    println!("→ Connecting to Acteon server at {}\n", base_url);

    let client = ActeonClient::new(&base_url);

    // =========================================================================
    // Health Check
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  HEALTH CHECK");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.health().await {
        Ok(true) => println!("  ✓ Server is healthy\n"),
        Ok(false) => {
            println!("  ✗ Server returned unhealthy status");
            println!("  Make sure acteon-server is running:");
            println!("    cargo run -p acteon-server\n");
            return Ok(());
        }
        Err(e) => {
            println!("  ✗ Failed to connect: {}", e);
            println!("\n  Make sure acteon-server is running:");
            println!("    cargo run -p acteon-server\n");
            return Ok(());
        }
    }

    // =========================================================================
    // List Loaded Rules
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  LOADED RULES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.list_rules().await {
        Ok(rules) => {
            if rules.is_empty() {
                println!("  No rules loaded (server running with defaults)\n");
            } else {
                println!("  {} rules loaded:", rules.len());
                for rule in &rules {
                    let status = if rule.enabled { "enabled" } else { "disabled" };
                    println!(
                        "    - {} (priority: {}, {})",
                        rule.name, rule.priority, status
                    );
                }
                println!();
            }
        }
        Err(e) => println!("  Failed to list rules: {}\n", e),
    }

    // =========================================================================
    // Single Action Dispatch
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SINGLE ACTION DISPATCH");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let action = Action::new(
        "notifications",
        "tenant-http-test",
        "email",
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Test from HTTP client",
            "body": "This action was dispatched via the REST API"
        }),
    );

    println!("  → POST /v1/dispatch");
    println!("    Action ID: {}", action.id);
    println!("    Provider: {}", action.provider);
    println!("    Type: {}", action.action_type);

    match client.dispatch(&action).await {
        Ok(outcome) => {
            println!("    Outcome: {:?}\n", outcome);
        }
        Err(e) => {
            println!("    Error: {}\n", e);
        }
    }

    // =========================================================================
    // Batch Dispatch
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  BATCH DISPATCH");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let batch_actions: Vec<Action> = (1..=5)
        .map(|i| {
            Action::new(
                "notifications",
                "tenant-http-test",
                "email",
                "batch_test",
                serde_json::json!({
                    "seq": i,
                    "message": format!("Batch message #{}", i)
                }),
            )
        })
        .collect();

    println!(
        "  → POST /v1/dispatch/batch ({} actions)",
        batch_actions.len()
    );

    match client.dispatch_batch(&batch_actions).await {
        Ok(results) => {
            let success_count = results
                .iter()
                .filter(|r| matches!(r, acteon_simulation::BatchResult::Success(_)))
                .count();
            let error_count = results.len() - success_count;

            println!("    Success: {}, Errors: {}\n", success_count, error_count);
        }
        Err(e) => {
            println!("    Error: {}\n", e);
        }
    }

    // =========================================================================
    // Test Rule Behaviors (if rules are loaded)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  TESTING RULE BEHAVIORS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Test suppression (if block-spam rule is loaded)
    println!("  Testing suppression (action_type='spam'):");
    let spam_action = Action::new(
        "notifications",
        "tenant-http-test",
        "email",
        "spam",
        serde_json::json!({"subject": "Buy now!!!"}),
    );

    match client.dispatch(&spam_action).await {
        Ok(outcome) => println!("    Outcome: {:?}", outcome),
        Err(e) => println!("    Result: {}", e),
    }

    // Test deduplication
    println!("\n  Testing deduplication (same dedup_key twice):");
    let dedup_action1 = Action::new(
        "notifications",
        "tenant-http-test",
        "email",
        "send_notification",
        serde_json::json!({"message": "First"}),
    )
    .with_dedup_key("http-dedup-test-key");

    let dedup_action2 = Action::new(
        "notifications",
        "tenant-http-test",
        "email",
        "send_notification",
        serde_json::json!({"message": "Second (should be deduped)"}),
    )
    .with_dedup_key("http-dedup-test-key");

    match client.dispatch(&dedup_action1).await {
        Ok(outcome) => println!("    First:  {:?}", outcome),
        Err(e) => println!("    First:  Error - {}", e),
    }

    match client.dispatch(&dedup_action2).await {
        Ok(outcome) => println!("    Second: {:?}", outcome),
        Err(e) => println!("    Second: Error - {}", e),
    }

    // =========================================================================
    // Query Audit Trail (if audit is enabled)
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let audit_query = AuditQuery {
        tenant: Some("tenant-http-test".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    match client.query_audit(&audit_query).await {
        Ok(page) => {
            if page.total == 0 {
                println!("  No audit records (audit may not be enabled)\n");
                println!("  To enable audit, use a config with [audit] section:");
                println!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
            } else {
                println!("  Found {} audit records for tenant-http-test:", page.total);
                for record in &page.records {
                    println!(
                        "    {} | {} | {} | {}",
                        &record.action_id[..8],
                        record.action_type,
                        record.outcome,
                        record.dispatched_at
                    );
                }
                println!();
            }
        }
        Err(e) => {
            println!("  Failed to query audit: {}", e);
            println!("  (Audit endpoint may not be available)\n");
        }
    }

    // =========================================================================
    // Throughput Test
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  THROUGHPUT TEST (100 sequential HTTP requests)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let start = std::time::Instant::now();
    let mut success = 0;
    let mut errors = 0;

    for i in 0..100 {
        let action = Action::new(
            "benchmark",
            "tenant-http-test",
            "email",
            "throughput_test",
            serde_json::json!({"seq": i}),
        );

        match client.dispatch(&action).await {
            Ok(_) => success += 1,
            Err(_) => errors += 1,
        }
    }

    let elapsed = start.elapsed();
    let throughput = 100.0 / elapsed.as_secs_f64();

    println!("  Completed: {} success, {} errors", success, errors);
    println!("  Duration: {:?}", elapsed);
    println!("  Throughput: {:.0} requests/sec\n", throughput);

    // =========================================================================
    // Summary
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  ARCHITECTURE RECAP");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  This simulation used HTTP calls to the Acteon server:");
    println!();
    println!("    ┌─────────────┐     HTTP POST      ┌─────────────────┐");
    println!("    │   Client    │ ───────────────────▶│  Acteon Server  │");
    println!("    │ (this code) │  /v1/dispatch      │  (acteon-server)│");
    println!("    └─────────────┘                    └────────┬────────┘");
    println!("                                                │");
    println!("                                                ▼");
    println!("                                       ┌─────────────────┐");
    println!("                                       │    Gateway      │");
    println!("                                       │  ┌───────────┐  │");
    println!("                                       │  │   Rules   │  │");
    println!("                                       │  │ (config)  │  │");
    println!("                                       │  └───────────┘  │");
    println!("                                       └────────┬────────┘");
    println!("                                                │");
    println!("                                                ▼");
    println!("                                       ┌─────────────────┐");
    println!("                                       │    Provider     │");
    println!("                                       │ (email, sms...) │");
    println!("                                       └─────────────────┘");
    println!();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║            HTTP CLIENT SIMULATION COMPLETE                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
