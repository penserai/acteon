//! Human approval over the credential-scoped REST API.
//!
//! Run a server configured with `examples/approval.toml`, then:
//! `cargo run -p acteon-simulation --example approval_simulation`.
//!
//! `ACTEON_URL` defaults to `http://localhost:8080`. With authentication enabled,
//! set `ACTEON_API_KEY` for dispatch and `ACTEON_OPERATOR_API_KEY` for decisions.
//! Both credentials must be provisioned for the same exact namespace and tenant.
//! The operator key must have the operator or admin role. With authentication
//! disabled, both clients use the fixed default/default context.
//!
//! The example rule requests approval for `process_refund` amounts over $1,000.
//! Approval IDs identify records within the authenticated context; no tenant
//! arguments or signed URL parameters are sent by the client.

use acteon_core::{ActionOutcome, ActionRequest};
use acteon_simulation::ActeonClient;
use tracing::info;

fn client(url: &str, key_variable: &str) -> Result<ActeonClient, acteon_client::Error> {
    let mut builder = ActeonClient::builder(url);
    if let Ok(key) = std::env::var(key_variable) {
        builder = builder.api_key(key);
    }
    builder.build()
}

fn refund(amount: u64, customer: &str) -> ActionRequest {
    ActionRequest::new(
        "payments",
        "process_refund",
        serde_json::json!({"amount": amount, "currency": "USD", "customer_id": customer}),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let url = std::env::var("ACTEON_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let dispatcher = client(&url, "ACTEON_API_KEY")?;
    let operator = client(&url, "ACTEON_OPERATOR_API_KEY")?;
    let dispatch_identity = dispatcher.identity().await?;
    let operator_identity = operator.identity().await?;
    if dispatch_identity.scope != operator_identity.scope {
        return Err("dispatch and operator credentials must have the same scope".into());
    }
    info!(scope = ?dispatch_identity.scope, "Authenticated context");

    let outcome = dispatcher.dispatch(&refund(50, "customer-small")).await?;
    info!(?outcome, "Small refund executes immediately");

    for (amount, customer, approve) in [
        (5_000, "customer-outage", true),
        (9_999, "customer-disputed", false),
    ] {
        let outcome = dispatcher.dispatch(&refund(amount, customer)).await?;
        let ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            ..
        } = outcome
        else {
            return Err("expected approval; load the large-refund approval rule".into());
        };
        info!(%approval_id, %expires_at, amount, "Refund awaiting decision");
        let pending = operator.list_approvals().await?;
        info!(count = pending.count, "Pending approvals in this context");
        let before = operator
            .get_approval(&approval_id)
            .await?
            .ok_or("pending approval was not found")?;
        assert_eq!(before.status, "pending");

        let decision = if approve {
            operator.approve(&approval_id).await?
        } else {
            operator.reject(&approval_id).await?
        };
        info!(status = %decision.status, outcome = ?decision.outcome, "Decision recorded");
        let after = operator
            .get_approval(&approval_id)
            .await?
            .ok_or("decided approval was not found")?;
        assert_eq!(after.status, if approve { "approved" } else { "rejected" });
    }
    Ok(())
}
