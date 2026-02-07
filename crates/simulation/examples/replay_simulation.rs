//! End-to-end HTTP simulation of action replay from the audit trail.
//!
//! This example dispatches actions via the REST API, queries the audit trail,
//! and replays selected actions to demonstrate the replay feature.
//!
//! Prerequisites:
//!   # Start the server with audit enabled (e.g. with PostgreSQL)
//!   docker compose --profile postgres up -d
//!   cargo run -p acteon-server -- -c examples/postgres.toml
//!
//! Then run this simulation:
//!   cargo run -p acteon-simulation --example replay_simulation
//!
//! Environment variables:
//!   ACTEON_URL - Server URL (default: http://localhost:8080)

use acteon_core::Action;
use acteon_simulation::{ActeonClient, AuditQuery, ReplayQuery};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          ACTION REPLAY SIMULATION DEMO                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let base_url =
        std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    println!("-> Connecting to Acteon server at {base_url}\n");

    let client = ActeonClient::new(&base_url);

    // =========================================================================
    // Health Check
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  HEALTH CHECK");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.health().await {
        Ok(true) => println!("  Server is healthy\n"),
        Ok(false) | Err(_) => {
            println!("  Server is not reachable. Make sure acteon-server is running");
            println!("  with audit enabled:");
            println!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
            return Ok(());
        }
    }

    // =========================================================================
    // STEP 1: Dispatch some actions to populate the audit trail
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  STEP 1: DISPATCH ACTIONS (populate audit trail)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut dispatched_ids = Vec::new();

    for i in 1..=3 {
        let action = Action::new(
            "notifications",
            "tenant-replay-demo",
            "email",
            "send_notification",
            serde_json::json!({
                "to": format!("user{i}@example.com"),
                "subject": format!("Test message #{i}"),
                "body": format!("This is test message number {i}")
            }),
        );

        let action_id = action.id.to_string();
        println!("  Dispatching action {i}: {}", &action_id[..8]);

        match client.dispatch(&action).await {
            Ok(outcome) => {
                println!("    Outcome: {outcome:?}");
                dispatched_ids.push(action_id);
            }
            Err(e) => println!("    Error: {e}"),
        }
    }
    println!();

    // =========================================================================
    // STEP 2: Query the audit trail
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  STEP 2: QUERY AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let audit_query = AuditQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    let page = match client.query_audit(&audit_query).await {
        Ok(page) => {
            if page.total == 0 {
                println!("  No audit records found. Audit may not be enabled.");
                println!("  Use a config with [audit] section:");
                println!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
                return Ok(());
            }

            println!("  Found {} audit records:", page.total);
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
            page
        }
        Err(e) => {
            println!("  Failed to query audit: {e}");
            println!("  (Audit endpoint may not be available)\n");
            return Ok(());
        }
    };

    // =========================================================================
    // STEP 3: Replay a single action
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  STEP 3: REPLAY SINGLE ACTION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    if let Some(record) = page.records.first() {
        let action_id = &record.action_id;
        println!("  Replaying action: {}...", &action_id[..8]);
        println!("  -> POST /v1/audit/{action_id}/replay");

        match client.replay_action(action_id).await {
            Ok(result) => {
                println!(
                    "    Original action: {}...",
                    &result.original_action_id[..8]
                );
                println!("    New action ID:   {}...", &result.new_action_id[..8]);
                println!("    Success: {}", result.success);
                if let Some(err) = &result.error {
                    println!("    Error: {err}");
                }
            }
            Err(e) => println!("    Error: {e}"),
        }
        println!();
    }

    // =========================================================================
    // STEP 4: Bulk replay with filters
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  STEP 4: BULK REPLAY WITH FILTERS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let replay_query = ReplayQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        action_type: Some("send_notification".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    println!("  Replaying all 'send_notification' actions for tenant-replay-demo");
    println!("  -> POST /v1/audit/replay?tenant=tenant-replay-demo&action_type=send_notification");

    match client.replay_audit(&replay_query).await {
        Ok(summary) => {
            println!("    Replayed:  {}", summary.replayed);
            println!("    Failed:    {}", summary.failed);
            println!("    Skipped:   {}", summary.skipped);
            println!("    Details:");
            for result in &summary.results {
                let status = if result.success { "OK" } else { "FAIL" };
                println!(
                    "      {}... -> {}... [{}]",
                    &result.original_action_id[..8],
                    &result.new_action_id[..8],
                    status
                );
            }
        }
        Err(e) => println!("    Error: {e}"),
    }
    println!();

    // =========================================================================
    // STEP 5: Verify replayed actions appear in the audit trail
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  STEP 5: VERIFY REPLAYED ACTIONS IN AUDIT TRAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let verify_query = AuditQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        limit: Some(20),
        ..Default::default()
    };

    match client.query_audit(&verify_query).await {
        Ok(page) => {
            let original_count = dispatched_ids.len();
            println!(
                "  Total audit records now: {} (was {original_count} originals)",
                page.total
            );
            println!("  Replayed actions have 'replayed_from' in their metadata.\n");
        }
        Err(e) => println!("  Failed to verify: {e}\n"),
    }

    // =========================================================================
    // Summary
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  HOW ACTION REPLAY WORKS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  1. Actions dispatched through the gateway are recorded in the");
    println!("     audit trail (when audit is enabled with store_payload: true).");
    println!();
    println!("  2. The replay endpoint reconstructs the original action from");
    println!("     the audit record and dispatches it through the full pipeline.");
    println!();
    println!("  3. Replayed actions receive a new UUID and include metadata:");
    println!("     replayed_from: <original-action-id>");
    println!();
    println!("  4. Bulk replay supports the same filters as the audit query:");
    println!("     namespace, tenant, provider, action_type, outcome, verdict,");
    println!("     matched_rule, and time range (from/to).");
    println!();
    println!("  Use cases:");
    println!("    - Replay actions that failed due to provider outage");
    println!("    - Re-execute suppressed actions after fixing rules");
    println!("    - Replay actions from the dead letter queue timeframe");
    println!();

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          ACTION REPLAY SIMULATION COMPLETE                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
