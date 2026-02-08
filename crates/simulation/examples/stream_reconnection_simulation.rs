//! SSE stream reconnection behavior simulation.
//!
//! This simulation demonstrates how clients can disconnect and reconnect
//! to the SSE event stream. It shows:
//!
//! 1. Client subscribes and receives events
//! 2. Client disconnects (drops receiver)
//! 3. Events continue being dispatched during the disconnect
//! 4. Client reconnects with a new subscription
//! 5. Missed events during the gap are lost (broadcast channel semantics)
//! 6. New events after reconnection are received normally
//!
//! In a real SSE deployment, `Last-Event-ID` can be used with server-side
//! event buffering to recover missed events. This simulation demonstrates
//! the raw broadcast channel behavior without server-side buffering.
//!
//! Run with: `cargo run -p acteon-simulation --example stream_reconnection_simulation`

use acteon_core::Action;
use acteon_simulation::prelude::*;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║      SSE RECONNECTION SIMULATION DEMO                       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Disconnect and Reconnect
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: DISCONNECT AND RECONNECT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A client connects, receives events, disconnects, then");
    println!("  reconnects. Events during the gap are missed.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();

    // Phase 1: Connected -- receive events.
    println!("  Phase 1: Client CONNECTED");
    let mut stream_rx = gateway.stream_tx().subscribe();

    let pre_actions: Vec<Action> = (0..3)
        .map(|i| {
            Action::new(
                "notifications",
                "tenant-1",
                "email",
                "send_email",
                serde_json::json!({"phase": "connected", "seq": i}),
            )
        })
        .collect();

    println!("  -> Dispatching 3 actions while connected...");
    for action in &pre_actions {
        harness.dispatch(action).await?;
    }

    let mut connected_events = Vec::new();
    while let Ok(event) = stream_rx.try_recv() {
        connected_events.push(event);
    }
    println!("  Received: {} events", connected_events.len());
    assert_eq!(
        connected_events.len(),
        3,
        "should receive all 3 events while connected"
    );

    // Phase 2: Disconnect -- drop the receiver.
    println!("\n  Phase 2: Client DISCONNECTED (dropping receiver)");
    drop(stream_rx);

    let gap_actions: Vec<Action> = (0..5)
        .map(|i| {
            Action::new(
                "notifications",
                "tenant-1",
                "email",
                "send_email",
                serde_json::json!({"phase": "disconnected", "seq": i}),
            )
        })
        .collect();

    println!("  -> Dispatching 5 actions while disconnected...");
    for action in &gap_actions {
        harness.dispatch(action).await?;
    }
    println!("  (Events emitted but no subscriber to receive them)");

    // Phase 3: Reconnect -- create a new subscriber.
    println!("\n  Phase 3: Client RECONNECTED (new subscription)");
    let mut stream_rx = gateway.stream_tx().subscribe();

    let post_actions: Vec<Action> = (0..4)
        .map(|i| {
            Action::new(
                "notifications",
                "tenant-1",
                "email",
                "send_email",
                serde_json::json!({"phase": "reconnected", "seq": i}),
            )
        })
        .collect();

    println!("  -> Dispatching 4 actions after reconnecting...");
    for action in &post_actions {
        harness.dispatch(action).await?;
    }

    let mut reconnected_events = Vec::new();
    while let Ok(event) = stream_rx.try_recv() {
        reconnected_events.push(event);
    }
    println!("  Received: {} events", reconnected_events.len());
    assert_eq!(
        reconnected_events.len(),
        4,
        "should receive only post-reconnect events"
    );

    // Summary.
    println!("\n  Summary:");
    println!(
        "    Phase 1 (connected):    3 dispatched, {} received",
        connected_events.len()
    );
    println!("    Phase 2 (disconnected): 5 dispatched, 0 received (missed)");
    println!(
        "    Phase 3 (reconnected):  4 dispatched, {} received",
        reconnected_events.len()
    );
    println!(
        "    Total provider calls:   {}",
        harness.provider("email").unwrap().call_count()
    );

    harness.teardown().await?;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Lagged Subscriber
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: LAGGED SUBSCRIBER (BACKPRESSURE)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  When a subscriber cannot keep up with the event rate, the");
    println!("  broadcast channel drops old events. The subscriber receives");
    println!("  a Lagged error and can recover by continuing to read.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Dispatch a large burst that may exceed the broadcast channel capacity.
    // The default tokio::sync::broadcast capacity is set when creating the channel.
    // We dispatch many events to test the lagged behavior.
    let burst_size = 200;
    println!("  Dispatching burst of {burst_size} actions without reading...");
    let burst_actions: Vec<Action> = (0..burst_size)
        .map(|i| {
            Action::new(
                "bulk",
                "tenant-1",
                "email",
                "burst",
                serde_json::json!({"seq": i}),
            )
        })
        .collect();

    for action in &burst_actions {
        harness.dispatch(action).await?;
    }

    // Now try to read -- we may get a Lagged error followed by remaining events.
    let mut received = 0;
    let mut lagged = false;
    loop {
        match stream_rx.try_recv() {
            Ok(_) => received += 1,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                println!("  Lagged! Missed {n} events due to slow consumption");
                lagged = true;
                // Continue reading remaining events.
            }
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
        }
    }

    println!("  Received after lag: {received} events");
    println!("  Lagged detected: {lagged}");
    println!("  Total dispatched: {burst_size}");

    if lagged {
        println!("\n  The subscriber was too slow and missed some events.");
        println!("  In production, clients use Last-Event-ID on reconnect");
        println!("  to recover missed events from server-side buffering.");
    } else {
        println!("\n  No lag detected -- broadcast channel capacity was sufficient.");
        println!("  (This depends on the channel size configured in the gateway.)");
    }

    harness.teardown().await?;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Multiple Reconnections
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: MULTIPLE RECONNECTIONS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A client connects and disconnects multiple times.");
    println!("  Each reconnection creates a fresh subscription.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();

    let mut total_received = 0;

    for round in 1..=4 {
        let mut rx = gateway.stream_tx().subscribe();

        let actions: Vec<Action> = (0..round)
            .map(|i| {
                Action::new(
                    "notifications",
                    "tenant-1",
                    "email",
                    "send_email",
                    serde_json::json!({"round": round, "seq": i}),
                )
            })
            .collect();

        for action in &actions {
            harness.dispatch(action).await?;
        }

        let mut count = 0;
        while let Ok(_event) = rx.try_recv() {
            count += 1;
        }

        println!(
            "  Round {round}: dispatched {}, received {count}",
            actions.len()
        );
        assert_eq!(count, actions.len());
        total_received += count;

        // Drop receiver to simulate disconnect.
        drop(rx);
    }

    println!("\n  Total events received across 4 rounds: {total_received}");
    assert_eq!(total_received, 1 + 2 + 3 + 4);

    harness.teardown().await?;
    println!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
