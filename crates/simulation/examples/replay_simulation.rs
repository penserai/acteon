//! End-to-end HTTP simulation of action replay from the audit trail.
//!
//! This example dispatches actions via the REST API, queries the audit trail,
//! and replays selected actions to demonstrate the replay feature.
//!
//! Prerequisites:
//!   # Start the server with the simulation config (log provider + audit)
//!   cargo run -p acteon-server -- -c examples/simulation.toml
//!
//!   # Or with PostgreSQL for persistent audit
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
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          ACTION REPLAY SIMULATION DEMO                       ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    let base_url =
        std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    info!("-> Connecting to Acteon server at {base_url}\n");

    let client = ActeonClient::new(&base_url);

    // =========================================================================
    // Health Check
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  HEALTH CHECK");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    match client.health().await {
        Ok(true) => info!("  Server is healthy\n"),
        Ok(false) | Err(_) => {
            info!("  Server is not reachable. Make sure acteon-server is running");
            info!("  with audit enabled:");
            info!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
            return Ok(());
        }
    }

    // =========================================================================
    // STEP 1: Dispatch some actions to populate the audit trail
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 1: DISPATCH ACTIONS (populate audit trail)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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
        info!("  Dispatching action {i}: {}", &action_id[..8]);

        match client.dispatch(&action).await {
            Ok(outcome) => {
                info!("    Outcome: {outcome:?}");
                dispatched_ids.push(action_id);
            }
            Err(e) => info!("    Error: {e}"),
        }
    }
    info!("");

    // =========================================================================
    // STEP 2: Query the audit trail
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 2: QUERY AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let audit_query = AuditQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    let page = match client.query_audit(&audit_query).await {
        Ok(page) => {
            if page.total.unwrap_or(0) == 0 && page.records.is_empty() {
                info!("  No audit records found. Audit may not be enabled.");
                info!("  Use a config with [audit] section:");
                info!("    cargo run -p acteon-server -- -c examples/postgres.toml\n");
                return Ok(());
            }

            info!("  Found {} audit records:", page.total.unwrap_or(0));
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
            page
        }
        Err(e) => {
            info!("  Failed to query audit: {e}");
            info!("  (Audit endpoint may not be available)\n");
            return Ok(());
        }
    };

    // =========================================================================
    // STEP 3: Replay a single action
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 3: REPLAY SINGLE ACTION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    if let Some(record) = page.records.first() {
        let action_id = &record.action_id;
        info!("  Replaying action: {}...", &action_id[..8]);
        info!("  -> POST /v1/audit/{action_id}/replay");

        match client.replay_action(action_id).await {
            Ok(result) => {
                info!(
                    "    Original action: {}...",
                    &result.original_action_id[..8]
                );
                info!("    New action ID:   {}...", &result.new_action_id[..8]);
                info!("    Success: {}", result.success);
                if let Some(err) = &result.error {
                    info!("    Error: {err}");
                }
            }
            Err(e) => info!("    Error: {e}"),
        }
        info!("");
    }

    // =========================================================================
    // STEP 4: Bulk replay with filters
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 4: BULK REPLAY WITH FILTERS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let replay_query = ReplayQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        action_type: Some("send_notification".to_string()),
        limit: Some(10),
        ..Default::default()
    };

    info!("  Replaying all 'send_notification' actions for tenant-replay-demo");
    info!("  -> POST /v1/audit/replay?tenant=tenant-replay-demo&action_type=send_notification");

    match client.replay_audit(&replay_query).await {
        Ok(summary) => {
            info!("    Replayed:  {}", summary.replayed);
            info!("    Failed:    {}", summary.failed);
            info!("    Skipped:   {}", summary.skipped);
            info!("    Details:");
            for result in &summary.results {
                let status = if result.success { "OK" } else { "FAIL" };
                info!(
                    "      {}... -> {}... [{}]",
                    &result.original_action_id[..8],
                    &result.new_action_id[..8],
                    status
                );
            }
        }
        Err(e) => info!("    Error: {e}"),
    }
    info!("");

    // =========================================================================
    // STEP 5: Verify replayed actions appear in the audit trail
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 5: VERIFY REPLAYED ACTIONS IN AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let verify_query = AuditQuery {
        tenant: Some("tenant-replay-demo".to_string()),
        limit: Some(20),
        ..Default::default()
    };

    match client.query_audit(&verify_query).await {
        Ok(page) => {
            let original_count = dispatched_ids.len();
            info!(
                "  Total audit records now: {} (was {original_count} originals)",
                page.total.unwrap_or(0)
            );
            info!("  Replayed actions have 'replayed_from' in their metadata.\n");
        }
        Err(e) => info!("  Failed to verify: {e}\n"),
    }

    // =========================================================================
    // Summary
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  HOW ACTION REPLAY WORKS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  1. Actions dispatched through the gateway are recorded in the");
    info!("     audit trail (when audit is enabled with store_payload: true).");
    info!("");
    info!("  2. The replay endpoint reconstructs the original action from");
    info!("     the audit record and dispatches it through the full pipeline.");
    info!("");
    info!("  3. Replayed actions receive a new UUID and include metadata:");
    info!("     replayed_from: <original-action-id>");
    info!("");
    info!("  4. Bulk replay supports the same filters as the audit query:");
    info!("     namespace, tenant, provider, action_type, outcome, verdict,");
    info!("     matched_rule, and time range (from/to).");
    info!("");
    info!("  Use cases:");
    info!("    - Replay actions that failed due to provider outage");
    info!("    - Re-execute suppressed actions after fixing rules");
    info!("    - Replay actions from the dead letter queue timeframe");
    info!("");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          ACTION REPLAY SIMULATION COMPLETE                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
