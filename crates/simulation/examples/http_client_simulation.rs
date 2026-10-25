//! End-to-end HTTP simulation against a running Acteon server.
//!
//! This example uses a generic HTTP client to dispatch actions via the REST API,
//! providing realistic integration testing.
//!
//! Prerequisites:
//!   # Start the server with the simulation config (log provider + rules)
//!   cargo run -p acteon-server -- -c examples/simulation.toml
//!
//!   # Or with a backend-specific config for audit persistence
//!   docker compose --profile postgres up -d
//!   cargo run -p acteon-server -- -c examples/postgres.toml
//!
//! Then run this simulation:
//!   cargo run -p acteon-simulation --example http_client_simulation
//!
//! Environment variables:
//!   ACTEON_URL - Server URL (default: http://localhost:8080)

use acteon_core::ActionRequest;
use acteon_simulation::{ActeonClient, AuditQuery};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║     HTTP CLIENT SIMULATION (End-to-End via REST API)         ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // Create HTTP client
    let base_url =
        std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    info!("→ Connecting to Acteon server at {}\n", base_url);

    let client = ActeonClient::new(&base_url);

    // =========================================================================
    // Health Check
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  HEALTH CHECK");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.health().await {
        Ok(true) => info!("  ✓ Server is healthy\n"),
        Ok(false) => {
            info!("  ✗ Server returned unhealthy status");
            info!("  Make sure acteon-server is running:");
            info!("    cargo run -p acteon-server\n");
            return Ok(());
        }
        Err(e) => {
            info!("  ✗ Failed to connect: {}", e);
            info!("\n  Make sure acteon-server is running:");
            info!("    cargo run -p acteon-server\n");
            return Ok(());
        }
    }

    // =========================================================================
    // List Loaded Rules
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  LOADED RULES");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.list_rules().await {
        Ok(rules) => {
            if rules.is_empty() {
                info!("  No rules loaded (server running with defaults)\n");
            } else {
                info!("  {} rules loaded:", rules.len());
                for rule in &rules {
                    let status = if rule.enabled { "enabled" } else { "disabled" };
                    info!(
                        "    - {} (priority: {}, {})",
                        rule.name, rule.priority, status
                    );
                }
                info!("");
            }
        }
        Err(e) => info!("  Failed to list rules: {}\n", e),
    }

    // =========================================================================
    // Single ActionRequest Dispatch
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SINGLE ACTION DISPATCH");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let action = ActionRequest::new(
        "email",
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Test from HTTP client",
            "body": "This action was dispatched via the REST API"
        }),
    );

    info!("  → POST /v1/dispatch");
    info!("    ActionRequest ID: {}", action.id);
    info!("    Provider: {}", action.provider);
    info!("    Type: {}", action.action_type);

    match client.dispatch(&action).await {
        Ok(outcome) => {
            info!("    Outcome: {:?}\n", outcome);
        }
        Err(e) => {
            info!("    Error: {}\n", e);
        }
    }

    // =========================================================================
    // Batch Dispatch
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  BATCH DISPATCH");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let batch_actions: Vec<ActionRequest> = (1..=5)
        .map(|i| {
            ActionRequest::new(
                "email",
                "batch_test",
                serde_json::json!({
                    "seq": i,
                    "message": format!("Batch message #{}", i)
                }),
            )
        })
        .collect();

    info!(
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

            info!("    Success: {}, Errors: {}\n", success_count, error_count);
        }
        Err(e) => {
            info!("    Error: {}\n", e);
        }
    }

    // =========================================================================
    // Test Rule Behaviors (if rules are loaded)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  TESTING RULE BEHAVIORS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Test suppression (if block-spam rule is loaded)
    info!("  Testing suppression (action_type='spam'):");
    let spam_action = ActionRequest::new(
        "email",
        "spam",
        serde_json::json!({"subject": "Buy now!!!"}),
    );

    match client.dispatch(&spam_action).await {
        Ok(outcome) => info!("    Outcome: {:?}", outcome),
        Err(e) => info!("    Result: {}", e),
    }

    // Test deduplication
    info!("\n  Testing deduplication (same dedup_key twice):");
    let dedup_action1 = ActionRequest::new(
        "email",
        "send_notification",
        serde_json::json!({"message": "First"}),
    )
    .with_dedup_key("http-dedup-test-key");

    let dedup_action2 = ActionRequest::new(
        "email",
        "send_notification",
        serde_json::json!({"message": "Second (should be deduped)"}),
    )
    .with_dedup_key("http-dedup-test-key");

    match client.dispatch(&dedup_action1).await {
        Ok(outcome) => info!("    First:  {:?}", outcome),
        Err(e) => info!("    First:  Error - {}", e),
    }

    match client.dispatch(&dedup_action2).await {
        Ok(outcome) => info!("    Second: {:?}", outcome),
        Err(e) => info!("    Second: Error - {}", e),
    }

    // =========================================================================
    // Query Audit Trail (if audit is enabled)
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let audit_query = AuditQuery {
        limit: Some(10),
        ..Default::default()
    };

    match client.query_audit(&audit_query).await {
        Ok(page) => {
            if page.total.unwrap_or(0) == 0 && page.records.is_empty() {
                info!("  No audit records (audit may not be enabled)\n");
                info!("  To enable audit, use a config with [audit] section:");
                info!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
            } else {
                info!(
                    "  Found {} audit records for tenant-http-test:",
                    page.total.unwrap_or(0)
                );
                for record in &page.records {
                    info!(
                        "    {} | {} | {} | {}",
                        &record.action_id[..8],
                        record.action_type,
                        record.outcome,
                        record.dispatched_at
                    );
                }
                info!("");
            }
        }
        Err(e) => {
            info!("  Failed to query audit: {}", e);
            info!("  (Audit endpoint may not be available)\n");
        }
    }

    // =========================================================================
    // Throughput Test
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  THROUGHPUT TEST (100 sequential HTTP requests)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let start = std::time::Instant::now();
    let mut success = 0;
    let mut errors = 0;

    for i in 0..100 {
        let action = ActionRequest::new("email", "throughput_test", serde_json::json!({"seq": i}));

        match client.dispatch(&action).await {
            Ok(_) => success += 1,
            Err(_) => errors += 1,
        }
    }

    let elapsed = start.elapsed();
    let throughput = 100.0 / elapsed.as_secs_f64();

    info!("  Completed: {} success, {} errors", success, errors);
    info!("  Duration: {:?}", elapsed);
    info!("  Throughput: {:.0} requests/sec\n", throughput);

    // =========================================================================
    // Summary
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  ARCHITECTURE RECAP");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  This simulation used HTTP calls to the Acteon server:");
    info!("");
    info!("    ┌─────────────┐     HTTP POST      ┌─────────────────┐");
    info!("    │   Client    │ ───────────────────▶│  Acteon Server  │");
    info!("    │ (this code) │  /v1/dispatch      │  (acteon-server)│");
    info!("    └─────────────┘                    └────────┬────────┘");
    info!("                                                │");
    info!("                                                ▼");
    info!("                                       ┌─────────────────┐");
    info!("                                       │    Gateway      │");
    info!("                                       │  ┌───────────┐  │");
    info!("                                       │  │   Rules   │  │");
    info!("                                       │  │ (config)  │  │");
    info!("                                       │  └───────────┘  │");
    info!("                                       └────────┬────────┘");
    info!("                                                │");
    info!("                                                ▼");
    info!("                                       ┌─────────────────┐");
    info!("                                       │    Provider     │");
    info!("                                       │ (email, sms...) │");
    info!("                                       └─────────────────┘");
    info!("");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║            HTTP CLIENT SIMULATION COMPLETE                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
