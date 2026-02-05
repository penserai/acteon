//! End-to-end simulation of the human-in-the-loop approval workflow via REST API.
//!
//! Demonstrates the full lifecycle using only public HTTP endpoints:
//! 1. Dispatch an action that falls below the approval threshold (immediate execution)
//! 2. Dispatch an action that requires human approval
//! 3. List pending approvals
//! 4. Approve the action via the signed URL parameters
//! 5. Dispatch another action requiring approval, then reject it
//!
//! Prerequisites:
//!   The server must be running with an approval rule loaded. Example rule YAML:
//!
//!   ```yaml
//!   rules:
//!     - name: approve-large-refunds
//!       priority: 1
//!       condition:
//!         all:
//!           - field: action.action_type
//!             eq: "process_refund"
//!           - field: action.payload.amount
//!             gt: 1000
//!       action:
//!         type: request_approval
//!         notify_provider: slack
//!         timeout_seconds: 3600
//!         message: "Refund over $1000 requires manager approval"
//!   ```
//!
//!   Start the server:
//!     cargo run -p acteon-server -- -c examples/approval.toml
//!
//!   Then run this simulation:
//!     cargo run -p acteon-simulation --example approval_simulation
//!
//! Environment variables:
//!   ACTEON_URL - Server URL (default: http://localhost:8080)
//!   ACTEON_API_KEY - API key for authenticated endpoints (optional)

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::ActeonClient;

/// Parse a query parameter value from a URL string.
fn parse_query_param(url: &str, param: &str) -> Option<String> {
    let query = url.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next()? == param {
            return kv.next().map(String::from);
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  HUMAN-IN-THE-LOOP APPROVAL SIMULATION (REST API)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let base_url =
        std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let api_key = std::env::var("ACTEON_API_KEY").ok();

    println!("  Connecting to Acteon server at {base_url}\n");

    let client = match api_key {
        Some(key) => ActeonClient::builder(&base_url)
            .api_key(key)
            .build()
            .expect("failed to build client"),
        None => ActeonClient::new(&base_url),
    };

    // =========================================================================
    // Health Check
    // =========================================================================
    println!("── Health check ──────────────────────────────────────────────\n");

    match client.health().await {
        Ok(true) => println!("  Server is healthy\n"),
        Ok(false) => {
            println!("  Server returned unhealthy status.");
            println!("  Make sure acteon-server is running with an approval rule loaded:");
            println!("    cargo run -p acteon-server -- -c examples/approval.toml\n");
            return Ok(());
        }
        Err(e) => {
            println!("  Failed to connect: {e}");
            println!("\n  Make sure acteon-server is running:");
            println!("    cargo run -p acteon-server\n");
            return Ok(());
        }
    }

    // =========================================================================
    // Step 1: Small refund ($50) — should execute immediately (no approval)
    // =========================================================================
    println!("── Step 1: Small refund (no approval needed) ──────────────────\n");

    let small_refund = Action::new(
        "billing",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({
            "amount": 50,
            "currency": "USD",
            "customer_id": "cust-123",
            "reason": "duplicate charge",
        }),
    );

    println!("  POST /v1/dispatch  (amount=50)");
    match client.dispatch(&small_refund).await {
        Ok(ActionOutcome::Executed(resp)) => {
            println!("  Outcome: EXECUTED (no approval needed)");
            println!("  Provider response: {:?}\n", resp.status);
        }
        Ok(other) => println!("  Outcome: {other:?}\n"),
        Err(e) => println!("  Error: {e}\n"),
    }

    // =========================================================================
    // Step 2: Large refund ($5000) — should require approval
    // =========================================================================
    println!("── Step 2: Large refund (approval required) ───────────────────\n");

    let large_refund = Action::new(
        "billing",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({
            "amount": 5000,
            "currency": "USD",
            "customer_id": "cust-456",
            "reason": "service outage compensation",
        }),
    );

    println!("  POST /v1/dispatch  (amount=5000)");
    let outcome = client.dispatch(&large_refund).await?;

    let (approval_id, approve_url) = match &outcome {
        ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            approve_url,
            reject_url,
            notification_sent,
        } => {
            println!("  Outcome: PENDING APPROVAL");
            println!("  Approval ID: {approval_id}");
            println!("  Expires at: {expires_at}");
            println!("  Notification sent: {notification_sent}");
            println!("  Approve URL: {approve_url}");
            println!("  Reject URL: {reject_url}");
            (approval_id.clone(), approve_url.clone())
        }
        other => {
            eprintln!("  ERROR: Expected PendingApproval, got: {other:?}");
            eprintln!("  Make sure the server has an approval rule for process_refund > $1000");
            return Err("unexpected outcome".into());
        }
    };

    // =========================================================================
    // Step 3: Check approval status via REST API
    // =========================================================================
    println!("\n── Step 3: Check approval status ──────────────────────────────\n");

    let sig = parse_query_param(&approve_url, "sig").expect("sig in approve URL");
    let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
        .expect("expires_at in approve URL")
        .parse()
        .expect("expires_at should be an integer");
    let kid = parse_query_param(&approve_url, "kid");

    println!("  GET /v1/approvals/billing/tenant-1/{approval_id}");
    match client
        .get_approval_with_kid(
            "billing",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await
    {
        Ok(Some(status)) => {
            println!("  Status: {}", status.status);
            println!("  Rule: {}", status.rule);
            if let Some(msg) = &status.message {
                println!("  Message: {msg}");
            }
        }
        Ok(None) => println!("  Approval not found (unexpected)"),
        Err(e) => println!("  Error: {e}"),
    }

    // =========================================================================
    // Step 4: List pending approvals (authenticated endpoint)
    // =========================================================================
    println!("\n── Step 4: List pending approvals ─────────────────────────────\n");

    println!("  GET /v1/approvals?namespace=billing&tenant=tenant-1");
    match client.list_approvals("billing", "tenant-1").await {
        Ok(list) => {
            println!("  Found {} pending approval(s):", list.count);
            for approval in &list.approvals {
                println!(
                    "    - {} | status={} | rule={}",
                    approval.token, approval.status, approval.rule
                );
            }
        }
        Err(e) => {
            println!("  Error listing approvals: {e}");
            println!("  (This endpoint requires authentication; set ACTEON_API_KEY)");
        }
    }

    // =========================================================================
    // Step 5: Approve the action (simulating a human clicking the link)
    // =========================================================================
    println!("\n── Step 5: Approve the action ─────────────────────────────────\n");

    println!("  Simulating human clicking 'Approve'...");
    println!("  HMAC signature: {}...", &sig[..16.min(sig.len())]);
    println!("  Expires at (unix): {expires_at}");

    println!("  POST /v1/approvals/billing/tenant-1/{approval_id}/approve");
    match client
        .approve_with_kid(
            "billing",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await
    {
        Ok(result) => {
            println!("  Result: status={}", result.status);
            if let Some(outcome) = &result.outcome {
                println!("  Outcome: {outcome}");
            }
            println!("  The original $5,000 refund has been executed!");
        }
        Err(e) => {
            eprintln!("  ERROR: {e}");
            return Err(e.into());
        }
    }

    // =========================================================================
    // Step 6: Verify approval status changed
    // =========================================================================
    println!("\n── Step 6: Verify approval is resolved ────────────────────────\n");

    println!("  GET /v1/approvals/billing/tenant-1/{approval_id}");
    match client
        .get_approval_with_kid(
            "billing",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await
    {
        Ok(Some(status)) => println!("  Status: {} (was: pending)", status.status),
        Ok(None) => println!("  Approval no longer found (cleaned up)"),
        Err(e) => println!("  Error: {e}"),
    }

    // =========================================================================
    // Step 7: Dispatch another large refund and reject it
    // =========================================================================
    println!("\n── Step 7: Reject a different refund ──────────────────────────\n");

    let another_refund = Action::new(
        "billing",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({
            "amount": 9999,
            "currency": "USD",
            "customer_id": "cust-789",
            "reason": "fraudulent charge",
        }),
    );

    println!("  POST /v1/dispatch  (amount=9999)");
    let outcome2 = client.dispatch(&another_refund).await?;

    let (approval_id_2, reject_url_2) = match &outcome2 {
        ActionOutcome::PendingApproval {
            approval_id,
            reject_url,
            ..
        } => {
            println!("  Outcome: PENDING APPROVAL (id={approval_id})");
            (approval_id.clone(), reject_url.clone())
        }
        other => {
            eprintln!("  ERROR: Expected PendingApproval, got: {other:?}");
            return Err("unexpected outcome".into());
        }
    };

    let reject_sig = parse_query_param(&reject_url_2, "sig").expect("sig in reject URL");
    let reject_expires: i64 = parse_query_param(&reject_url_2, "expires_at")
        .expect("expires_at in reject URL")
        .parse()
        .expect("expires_at should be an integer");
    let reject_kid = parse_query_param(&reject_url_2, "kid");

    println!("  Simulating human clicking 'Reject'...");
    println!("  POST /v1/approvals/billing/tenant-1/{approval_id_2}/reject");
    match client
        .reject_with_kid(
            "billing",
            "tenant-1",
            &approval_id_2,
            &reject_sig,
            reject_expires,
            reject_kid.as_deref(),
        )
        .await
    {
        Ok(result) => {
            println!("  Result: status={}", result.status);
            println!("  The $9,999 refund was NOT executed.");
        }
        Err(e) => {
            eprintln!("  ERROR: {e}");
            return Err(e.into());
        }
    }

    // =========================================================================
    // Done
    // =========================================================================
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SIMULATION COMPLETE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    Ok(())
}
