//! Multiple concurrent SSE subscribers with different filter configurations.
//!
//! This simulation demonstrates how multiple clients can subscribe to the
//! same gateway event stream and independently filter events by namespace,
//! tenant, and action type. It shows:
//!
//! 1. Multiple subscribers receiving events simultaneously
//! 2. Client-side filtering for namespace and tenant isolation
//! 3. Burst of actions across multiple namespaces and tenants
//! 4. Each subscriber seeing only matching events
//!
//! Run with: `cargo run -p acteon-simulation --example multi_client_stream_simulation`

use acteon_core::{Action, StreamEvent};
use acteon_simulation::prelude::*;
use tokio::sync::broadcast;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║      MULTI-CLIENT SSE STREAM SIMULATION DEMO                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Multiple Subscribers, Same Stream
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: MULTIPLE SUBSCRIBERS, SAME STREAM");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Three subscribers connect to the same broadcast channel.");
    println!("  All three receive every event (fan-out behavior).\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("slack")
            .add_recording_provider("webhook")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();

    // Create three independent subscribers.
    let mut sub_a = gateway.stream_tx().subscribe();
    let mut sub_b = gateway.stream_tx().subscribe();
    let mut sub_c = gateway.stream_tx().subscribe();

    println!("  Subscribed 3 clients: A, B, C");

    // Dispatch a few actions.
    let actions = vec![
        Action::new(
            "billing",
            "acme",
            "email",
            "invoice",
            serde_json::json!({"amount": 250.00}),
        ),
        Action::new(
            "alerts",
            "acme",
            "slack",
            "post_alert",
            serde_json::json!({"severity": "warning"}),
        ),
    ];

    println!("  Dispatching {} actions...\n", actions.len());
    for action in &actions {
        harness.dispatch(action).await?;
    }

    // Drain each subscriber and count.
    let count_a = drain_events(&mut sub_a).len();
    let count_b = drain_events(&mut sub_b).len();
    let count_c = drain_events(&mut sub_c).len();

    println!("  Subscriber A received: {count_a} events");
    println!("  Subscriber B received: {count_b} events");
    println!("  Subscriber C received: {count_c} events");

    assert_eq!(count_a, 2);
    assert_eq!(count_b, 2);
    assert_eq!(count_c, 2);
    println!("  All 3 subscribers received all 2 events (fan-out confirmed)");

    harness.teardown().await?;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Filtered Subscribers (Namespace Isolation)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: NAMESPACE-FILTERED SUBSCRIBERS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Two subscribers filter events by namespace.");
    println!("  Each only sees events for their namespace of interest.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("slack")
            .add_recording_provider("webhook")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut billing_sub = gateway.stream_tx().subscribe();
    let mut alerts_sub = gateway.stream_tx().subscribe();

    println!("  Subscriber 'billing-watcher': filter namespace=billing");
    println!("  Subscriber 'alerts-watcher':  filter namespace=alerts\n");

    // Dispatch actions across multiple namespaces.
    let mixed = vec![
        Action::new(
            "billing",
            "acme",
            "email",
            "invoice",
            serde_json::json!({"amount": 100}),
        ),
        Action::new(
            "alerts",
            "acme",
            "slack",
            "post_alert",
            serde_json::json!({"severity": "critical"}),
        ),
        Action::new(
            "billing",
            "acme",
            "email",
            "receipt",
            serde_json::json!({"amount": 50}),
        ),
        Action::new(
            "notifications",
            "acme",
            "webhook",
            "welcome",
            serde_json::json!({"user": "charlie"}),
        ),
        Action::new(
            "alerts",
            "acme",
            "slack",
            "post_alert",
            serde_json::json!({"severity": "info"}),
        ),
    ];

    println!(
        "  Dispatching {} actions across billing/alerts/notifications",
        mixed.len()
    );
    for action in &mixed {
        harness.dispatch(action).await?;
    }

    // Apply client-side namespace filters.
    let billing_events: Vec<_> = drain_events(&mut billing_sub)
        .into_iter()
        .filter(|e| e.namespace == "billing")
        .collect();
    let alerts_events: Vec<_> = drain_events(&mut alerts_sub)
        .into_iter()
        .filter(|e| e.namespace == "alerts")
        .collect();

    println!(
        "\n  billing-watcher received: {} events",
        billing_events.len()
    );
    for event in &billing_events {
        println!(
            "    ns={:<16} type={:<16} action={}",
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
            event
                .action_id
                .as_deref()
                .map_or("-".into(), |id| id[..8.min(id.len())].to_string()),
        );
    }

    println!(
        "\n  alerts-watcher received: {} events",
        alerts_events.len()
    );
    for event in &alerts_events {
        println!(
            "    ns={:<16} type={:<16} action={}",
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
            event
                .action_id
                .as_deref()
                .map_or("-".into(), |id| id[..8.min(id.len())].to_string()),
        );
    }

    assert_eq!(
        billing_events.len(),
        2,
        "billing watcher should see 2 events"
    );
    assert_eq!(alerts_events.len(), 2, "alerts watcher should see 2 events");

    harness.teardown().await?;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Tenant Isolation
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: TENANT ISOLATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Subscribers filter by tenant to enforce data isolation.");
    println!("  Events from one tenant are invisible to another.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut acme_sub = gateway.stream_tx().subscribe();
    let mut globex_sub = gateway.stream_tx().subscribe();

    println!("  Subscriber 'acme-monitor':   filter tenant=acme");
    println!("  Subscriber 'globex-monitor': filter tenant=globex\n");

    // Dispatch actions for different tenants.
    let tenant_actions = vec![
        Action::new(
            "notifications",
            "acme",
            "email",
            "send_email",
            serde_json::json!({"to": "alice@acme.com"}),
        ),
        Action::new(
            "notifications",
            "globex",
            "email",
            "send_email",
            serde_json::json!({"to": "bob@globex.com"}),
        ),
        Action::new(
            "notifications",
            "acme",
            "email",
            "send_email",
            serde_json::json!({"to": "carol@acme.com"}),
        ),
        Action::new(
            "notifications",
            "globex",
            "email",
            "send_email",
            serde_json::json!({"to": "dave@globex.com"}),
        ),
        Action::new(
            "notifications",
            "acme",
            "email",
            "send_email",
            serde_json::json!({"to": "eve@acme.com"}),
        ),
    ];

    println!(
        "  Dispatching {} actions (3 acme, 2 globex)",
        tenant_actions.len()
    );
    for action in &tenant_actions {
        harness.dispatch(action).await?;
    }

    let acme_events: Vec<_> = drain_events(&mut acme_sub)
        .into_iter()
        .filter(|e| e.tenant == "acme")
        .collect();
    let globex_events: Vec<_> = drain_events(&mut globex_sub)
        .into_iter()
        .filter(|e| e.tenant == "globex")
        .collect();

    println!("\n  acme-monitor received:   {} events", acme_events.len());
    println!("  globex-monitor received: {} events", globex_events.len());

    assert_eq!(acme_events.len(), 3, "acme should see 3 events");
    assert_eq!(globex_events.len(), 2, "globex should see 2 events");

    // Verify no cross-contamination.
    for event in &acme_events {
        assert_eq!(event.tenant, "acme");
    }
    for event in &globex_events {
        assert_eq!(event.tenant, "globex");
    }
    println!("  No cross-tenant contamination detected");

    harness.teardown().await?;
    println!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // SCENARIO 4: Burst of Actions with Concurrent Subscribers
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: ACTION BURST WITH CONCURRENT SUBSCRIBERS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Dispatch a burst of 50 actions while 3 subscribers are active.");
    println!("  Verify all subscribers receive the full burst.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut sub_1 = gateway.stream_tx().subscribe();
    let mut sub_2 = gateway.stream_tx().subscribe();
    let mut sub_3 = gateway.stream_tx().subscribe();

    let burst_size = 50;
    let burst_actions: Vec<Action> = (0..burst_size)
        .map(|i| {
            let tenant = if i % 2 == 0 { "even-corp" } else { "odd-corp" };
            Action::new(
                "bulk",
                tenant,
                "email",
                "burst_email",
                serde_json::json!({"seq": i}),
            )
        })
        .collect();

    println!("  Dispatching burst of {burst_size} actions...");
    let start = std::time::Instant::now();
    for action in &burst_actions {
        harness.dispatch(action).await?;
    }
    let elapsed = start.elapsed();
    println!("  Burst dispatched in {elapsed:?}");

    let events_1 = drain_events(&mut sub_1);
    let events_2 = drain_events(&mut sub_2);
    let events_3 = drain_events(&mut sub_3);

    println!("\n  Subscriber 1: {} events", events_1.len());
    println!("  Subscriber 2: {} events", events_2.len());
    println!("  Subscriber 3: {} events", events_3.len());

    assert_eq!(events_1.len(), burst_size);
    assert_eq!(events_2.len(), burst_size);
    assert_eq!(events_3.len(), burst_size);

    // Show tenant breakdown for subscriber 1.
    let even_count = events_1.iter().filter(|e| e.tenant == "even-corp").count();
    let odd_count = events_1.iter().filter(|e| e.tenant == "odd-corp").count();
    println!("\n  Tenant breakdown (sub 1): even-corp={even_count}, odd-corp={odd_count}");

    harness.teardown().await?;
    println!("\n  [Scenario 4 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

/// Drain all currently available events from a broadcast receiver.
fn drain_events(rx: &mut broadcast::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}
