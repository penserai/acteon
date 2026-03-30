//! Basic demonstration of SSE event streaming in Acteon.
//!
//! This simulation shows how to subscribe to the gateway's broadcast channel
//! and observe real-time events as actions are dispatched. It demonstrates:
//!
//! 1. Subscribing to the event stream before dispatching actions
//! 2. Dispatching actions of different types (email, slack, webhook)
//! 3. Receiving events in real-time via the broadcast channel
//! 4. Filtering events by namespace and action type
//!
//! Run with: `cargo run -p acteon-simulation --example sse_stream_simulation`

use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_simulation::prelude::*;
use tracing::info;

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
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          SSE EVENT STREAM SIMULATION DEMO                   ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Basic Event Streaming
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: BASIC EVENT STREAMING");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Subscribe to the gateway event stream and dispatch actions.");
    info!("  Each dispatch emits a StreamEvent with full metadata.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("slack")
            .add_recording_provider("webhook")
            .build(),
    )
    .await?;

    info!("  Started simulation cluster with 1 node");
    info!("  Registered providers: email, slack, webhook\n");

    // Subscribe to the event stream via the gateway broadcast channel.
    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Dispatch several actions of different types.
    let actions = [
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({
                "to": "alice@example.com",
                "subject": "Welcome aboard",
            }),
        ),
        Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "post_message",
            serde_json::json!({
                "channel": "#ops",
                "message": "Deployment successful",
            }),
        ),
        Action::new(
            "integrations",
            "tenant-1",
            "webhook",
            "fire_webhook",
            serde_json::json!({
                "url": "https://hooks.example.com/deploy",
                "payload": {"status": "ok"},
            }),
        ),
    ];

    for (i, action) in actions.iter().enumerate() {
        info!(
            "  -> Dispatching action {} [{}/{}] (provider={}, type={})",
            &action.id.to_string()[..8],
            i + 1,
            actions.len(),
            action.provider,
            action.action_type,
        );
        let outcome = harness.dispatch(action).await?;
        info!("     Outcome: {}", outcome_summary(&outcome));
    }

    // Read all events from the stream.
    info!("\n  Events received on stream:");
    let mut event_count = 0;
    while let Ok(event) = stream_rx.try_recv() {
        event_count += 1;
        print_event(event_count, &event);
    }

    assert_eq!(event_count, 3, "expected 3 stream events");
    info!("\n  Total events received: {event_count}");

    harness.teardown().await?;
    info!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Suppressed Actions Still Emit Events
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: SUPPRESSED ACTIONS EMIT EVENTS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Even suppressed actions produce stream events, so monitoring");
    info!("  dashboards can track rule enforcement in real time.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(SUPPRESSION_RULE)
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Send spam (will be suppressed) and a legitimate action.
    let spam = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({"subject": "Buy now!!!"}),
    );
    let legit = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"subject": "Your order shipped"}),
    );

    info!("  -> Dispatching SPAM action (should be suppressed)...");
    let outcome = harness.dispatch(&spam).await?;
    info!("     Outcome: {}", outcome_summary(&outcome));

    info!("  -> Dispatching LEGITIMATE action...");
    let outcome = harness.dispatch(&legit).await?;
    info!("     Outcome: {}", outcome_summary(&outcome));

    info!("\n  Events received on stream:");
    let mut event_count = 0;
    while let Ok(event) = stream_rx.try_recv() {
        event_count += 1;
        print_event(event_count, &event);
    }

    assert_eq!(
        event_count, 2,
        "both dispatches should emit events (even suppressed)"
    );
    info!("\n  Both actions emitted events -- suppression is visible on the stream");
    info!(
        "  Provider calls: {} (only the legit action reached the provider)",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    info!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Namespace and Action-Type Filtering
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: CLIENT-SIDE FILTERING");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Clients can filter events by namespace, tenant, and action type.");
    info!("  This scenario dispatches actions across multiple namespaces and");
    info!("  then filters the stream to show only matching events.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("slack")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Dispatch actions across different namespaces.
    let mixed_actions = [
        Action::new(
            "billing",
            "tenant-1",
            "email",
            "invoice",
            serde_json::json!({"amount": 99.99}),
        ),
        Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "post_alert",
            serde_json::json!({"severity": "warning"}),
        ),
        Action::new(
            "billing",
            "tenant-1",
            "email",
            "receipt",
            serde_json::json!({"amount": 49.99}),
        ),
        Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "post_alert",
            serde_json::json!({"severity": "critical"}),
        ),
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "welcome",
            serde_json::json!({"user": "bob"}),
        ),
    ];

    info!(
        "  -> Dispatching {} actions across billing/alerts/notifications",
        mixed_actions.len()
    );
    for action in &mixed_actions {
        harness.dispatch(action).await?;
    }

    // Collect all events.
    let mut all_events = Vec::new();
    while let Ok(event) = stream_rx.try_recv() {
        all_events.push(event);
    }

    info!("  Total events: {}\n", all_events.len());

    // Filter: only billing namespace.
    let billing_events: Vec<_> = all_events
        .iter()
        .filter(|e| e.namespace == "billing")
        .collect();
    info!(
        "  Filter: namespace=billing -> {} events",
        billing_events.len()
    );
    for event in &billing_events {
        info!(
            "    [{:>12}] ns={:<16} type={}",
            event_type_label(&event.event_type),
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
        );
    }
    assert_eq!(billing_events.len(), 2);

    // Filter: only alerts namespace with action_type=post_alert.
    let alert_events: Vec<_> = all_events
        .iter()
        .filter(|e| e.namespace == "alerts" && e.action_type.as_deref() == Some("post_alert"))
        .collect();
    info!(
        "\n  Filter: namespace=alerts, action_type=post_alert -> {} events",
        alert_events.len()
    );
    for event in &alert_events {
        info!(
            "    [{:>12}] ns={:<16} type={}",
            event_type_label(&event.event_type),
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
        );
    }
    assert_eq!(alert_events.len(), 2);

    harness.teardown().await?;
    info!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              ALL SCENARIOS PASSED                           ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Print a single stream event in a formatted line.
fn print_event(index: usize, event: &StreamEvent) {
    let type_label = event_type_label(&event.event_type);
    info!(
        "    #{index}: [{type_label:>12}] ns={:<16} tenant={:<12} action_type={:<16} id={}",
        event.namespace,
        event.tenant,
        event.action_type.as_deref().unwrap_or("-"),
        event
            .action_id
            .as_deref()
            .map_or_else(|| "-".to_string(), |id| id[..8.min(id.len())].to_string()),
    );
}

/// Get a short label for the event type.
fn event_type_label(event_type: &StreamEventType) -> &'static str {
    match event_type {
        StreamEventType::ActionDispatched { .. } => "dispatched",
        StreamEventType::GroupFlushed { .. } => "group_flush",
        StreamEventType::Timeout { .. } => "timeout",
        StreamEventType::ChainAdvanced { .. } => "chain_step",
        StreamEventType::ApprovalRequired { .. } => "approval",
        StreamEventType::ScheduledActionDue { .. } => "scheduled",
        StreamEventType::ChainStepCompleted { .. } => "chain_step_done",
        StreamEventType::ChainCompleted { .. } => "chain_done",
        StreamEventType::GroupEventAdded { .. } => "group_event",
        StreamEventType::GroupResolved { .. } => "group_resolved",
        StreamEventType::ApprovalResolved { .. } => "approval_resolved",
        StreamEventType::Unknown => "unknown",
    }
}

/// Produce a short human-readable summary of an `ActionOutcome`.
fn outcome_summary(outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Executed(_) => "EXECUTED".to_string(),
        ActionOutcome::Failed(err) => format!("FAILED ({})", err.message),
        ActionOutcome::Suppressed { rule } => format!("SUPPRESSED (rule: {rule})"),
        ActionOutcome::Deduplicated => "DEDUPLICATED".to_string(),
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            ..
        } => format!("REROUTED ({original_provider} -> {new_provider})"),
        ActionOutcome::CircuitOpen { provider, .. } => {
            format!("CIRCUIT_OPEN (provider: {provider})")
        }
        other => format!("{other:?}"),
    }
}
