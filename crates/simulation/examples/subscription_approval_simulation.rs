//! Approval workflow subscription simulation — subscribe to an action that
//! requires human approval and observe the full approval lifecycle.
//!
//! Demonstrates:
//! 1. Dispatching an action that triggers `request_approval` rule
//! 2. Receiving `ActionDispatched` event with `PendingApproval` outcome
//! 3. Executing the approval via the gateway API
//! 4. Observing the approved action's execution outcome on the stream
//! 5. Rejecting an approval and observing the rejection
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_approval_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const APPROVAL_RULE: &str = r#"
rules:
  - name: approve-large-refunds
    priority: 1
    condition:
      field: action.action_type
      eq: "process_refund"
    action:
      type: request_approval
      notify_provider: notifier
      timeout_seconds: 3600
      message: "Large refund requires manager approval"
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

fn event_type_label(event_type: &StreamEventType) -> &'static str {
    match event_type {
        StreamEventType::ActionDispatched { .. } => "dispatched",
        StreamEventType::GroupFlushed { .. } => "group_flush",
        StreamEventType::Timeout { .. } => "timeout",
        StreamEventType::ChainAdvanced { .. } => "chain_advanced",
        StreamEventType::ApprovalRequired { .. } => "approval",
        StreamEventType::ScheduledActionDue { .. } => "scheduled",
        StreamEventType::ChainStepCompleted { .. } => "step_completed",
        StreamEventType::ChainCompleted { .. } => "chain_completed",
        StreamEventType::GroupEventAdded { .. } => "group_added",
        StreamEventType::GroupResolved { .. } => "group_resolved",
        StreamEventType::ApprovalResolved { .. } => "approval_resolved",
        StreamEventType::Unknown => "unknown",
    }
}

/// Parse a query parameter from a URL.
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

type ApprovalGatewayResult = Result<
    (
        acteon_gateway::Gateway,
        Arc<RecordingProvider>,
        Arc<RecordingProvider>,
    ),
    Box<dyn std::error::Error>,
>;

/// Build a gateway configured for approval workflows.
fn build_approval_gateway() -> ApprovalGatewayResult {
    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let rules = parse_rules(APPROVAL_RULE);

    // The payment provider executes the actual refund after approval.
    let payment_provider = Arc::new(RecordingProvider::new("payments").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "refund_id": "REF-42",
            "status": "processed"
        })))
    }));

    // The notifier provider sends approval notification emails.
    let notifier_provider = Arc::new(RecordingProvider::new("notifier"));

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .rules(rules)
        .provider(Arc::clone(&payment_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&notifier_provider) as Arc<dyn acteon_provider::DynProvider>)
        .approval_secret(b"simulation-secret-key-for-demo!")
        .external_url("https://approvals.example.com")
        .build()?;

    Ok((gateway, payment_provider, notifier_provider))
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     APPROVAL SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: Approval granted — full lifecycle
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: APPROVAL GRANTED");
    println!("------------------------------------------------------------------\n");

    println!("  A refund action triggers an approval workflow. A subscriber");
    println!("  watches the pending state, then observes the approval outcome.\n");

    let (gateway, payment_provider, notifier_provider) = build_approval_gateway()?;
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Dispatch a refund that requires approval.
    let refund = Action::new(
        "payments",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({
            "order_id": "ORD-456",
            "amount": 250.00,
            "reason": "Customer request"
        }),
    );

    println!("  -> Dispatching refund action (requires approval)...");
    let outcome = gateway.dispatch(refund, None).await?;

    let (approval_id, approve_url) = match &outcome {
        ActionOutcome::PendingApproval {
            approval_id,
            approve_url,
            expires_at,
            notification_sent,
            ..
        } => {
            println!("     Status: PENDING APPROVAL");
            println!("     Approval ID: {approval_id}");
            println!("     Expires at: {expires_at}");
            println!("     Notification sent: {notification_sent}");
            (approval_id.clone(), approve_url.clone())
        }
        other => panic!("Expected PendingApproval, got {other:?}"),
    };

    // Check subscription events — should have the PendingApproval dispatch event.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    println!("\n  Subscription events after dispatch:");
    for event in &events {
        match &event.event_type {
            StreamEventType::ActionDispatched { outcome, provider } => {
                let category = acteon_core::outcome_category(outcome);
                println!(
                    "    [dispatched] provider={provider} outcome={category} action_id={}",
                    event.action_id.as_deref().unwrap_or("-")
                );
            }
            _ => {
                println!("    [{:>15}]", event_type_label(&event.event_type));
            }
        }
    }

    assert!(
        !events.is_empty(),
        "should have at least one dispatch event"
    );

    // Verify the notifier was called (approval notification sent).
    println!(
        "\n  Notifier calls: {} (approval notification)",
        notifier_provider.call_count()
    );
    assert_eq!(
        notifier_provider.call_count(),
        1,
        "notification should be sent"
    );

    // Now simulate the manager approving the refund.
    println!("\n  -> Manager approves the refund...");
    let sig = parse_query_param(&approve_url, "sig").expect("sig param");
    let expires_at: i64 = parse_query_param(&approve_url, "expires_at")
        .expect("expires_at param")
        .parse()
        .expect("expires_at should be an integer");
    let kid = parse_query_param(&approve_url, "kid");

    let approval_result = gateway
        .execute_approval(
            "payments",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await?;

    match &approval_result {
        ActionOutcome::Executed(resp) => {
            println!("     Approval executed: status={:?}", resp.status);
        }
        other => {
            println!("     Approval result: {other:?}");
        }
    }

    // Check subscription events after approval.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let post_approval_events = drain_events(&mut stream_rx);

    println!("\n  Subscription events after approval:");
    for event in &post_approval_events {
        println!(
            "    [{:>15}] action_id={}",
            event_type_label(&event.event_type),
            event.action_id.as_deref().unwrap_or("-"),
        );
    }

    // Verify payment provider was called (the refund was executed).
    println!(
        "\n  Payment provider calls: {} (refund executed after approval)",
        payment_provider.call_count()
    );
    assert_eq!(
        payment_provider.call_count(),
        1,
        "payment should be executed after approval"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Approval rejected
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: APPROVAL REJECTED");
    println!("------------------------------------------------------------------\n");

    println!("  A refund is dispatched, then rejected by the manager.");
    println!("  The subscriber sees the rejection and verifies no execution.\n");

    let (gateway, payment_provider, _notifier) = build_approval_gateway()?;
    let mut stream_rx = gateway.stream_tx().subscribe();

    let refund = Action::new(
        "payments",
        "tenant-1",
        "payments",
        "process_refund",
        serde_json::json!({
            "order_id": "ORD-789",
            "amount": 500.00,
            "reason": "Suspected fraud"
        }),
    );

    println!("  -> Dispatching refund action...");
    let outcome = gateway.dispatch(refund, None).await?;

    let (approval_id, reject_url) = match &outcome {
        ActionOutcome::PendingApproval {
            approval_id,
            reject_url,
            ..
        } => {
            println!("     Status: PENDING APPROVAL");
            println!("     Approval ID: {approval_id}");
            (approval_id.clone(), reject_url.clone())
        }
        other => panic!("Expected PendingApproval, got {other:?}"),
    };

    // Drain initial events.
    tokio::time::sleep(Duration::from_millis(30)).await;
    drain_events(&mut stream_rx);

    // Manager rejects the refund.
    println!("\n  -> Manager rejects the refund...");
    let sig = parse_query_param(&reject_url, "sig").expect("sig param");
    let expires_at: i64 = parse_query_param(&reject_url, "expires_at")
        .expect("expires_at param")
        .parse()
        .expect("expires_at should be an integer");
    let kid = parse_query_param(&reject_url, "kid");

    gateway
        .reject_approval(
            "payments",
            "tenant-1",
            &approval_id,
            &sig,
            expires_at,
            kid.as_deref(),
        )
        .await?;

    println!("     Rejection completed successfully");

    // Check post-rejection events.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let post_rejection_events = drain_events(&mut stream_rx);

    println!("\n  Subscription events after rejection:");
    if post_rejection_events.is_empty() {
        println!("    (no new events -- rejection is terminal)");
    } else {
        for event in &post_rejection_events {
            println!("    [{:>15}]", event_type_label(&event.event_type),);
        }
    }

    // Verify payment provider was NOT called.
    println!(
        "\n  Payment provider calls: {} (refund was never executed)",
        payment_provider.call_count()
    );
    assert_eq!(
        payment_provider.call_count(),
        0,
        "rejected refund should not execute"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("==================================================================");
    println!("              ALL SCENARIOS PASSED");
    println!("==================================================================");

    Ok(())
}
