//! State machine subscription simulation — subscribe to actions that go
//! through state machine transitions and observe state changes in real time.
//!
//! Demonstrates:
//! 1. Configuring a state machine with states and transitions
//! 2. Dispatching actions that trigger state transitions
//! 3. Receiving `ActionDispatched` events with `StateChanged` outcomes
//! 4. Tracking state progression through subscription events
//! 5. Notify vs no-notify transitions visible in the stream
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_state_machine_simulation`

use acteon_core::{
    Action, ActionOutcome, StateMachineConfig, StreamEvent, StreamEventType, TransitionConfig,
    TransitionEffects,
};
use acteon_simulation::prelude::*;

/// State machine rule: route "ticket" actions through the ticket lifecycle.
const TICKET_RULE: &str = r"
rules:
  - name: ticket-lifecycle
    priority: 5
    condition:
      field: action.action_type
      eq: ticket
    action:
      type: state_machine
      state_machine: ticket
      fingerprint_fields:
        - action_type
        - payload.ticket_id
";

/// Build a ticket state machine:
///   `new` -> `open` -> `in_progress` -> `review` -> `closed`
///
/// Transitions:
///   `new` -> `open` (notify)
///   `open` -> `in_progress` (no notify)
///   `in_progress` -> `review` (notify)
///   `review` -> `closed` (notify)
///   `review` -> `in_progress` (no notify, rework)
fn ticket_state_machine() -> StateMachineConfig {
    StateMachineConfig::new("ticket", "new")
        .with_state("open")
        .with_state("in_progress")
        .with_state("review")
        .with_state("closed")
        .with_transition(
            TransitionConfig::new("new", "open").with_effects(TransitionEffects::notify()),
        )
        .with_transition(TransitionConfig::new("open", "in_progress"))
        .with_transition(
            TransitionConfig::new("in_progress", "review")
                .with_effects(TransitionEffects::notify()),
        )
        .with_transition(
            TransitionConfig::new("review", "closed").with_effects(TransitionEffects::notify()),
        )
        .with_transition(TransitionConfig::new("review", "in_progress"))
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

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     STATE MACHINE SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: Ticket lifecycle — full progression
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: TICKET LIFECYCLE (new -> ... -> closed)");
    println!("------------------------------------------------------------------\n");

    println!("  A ticket goes through: new -> open -> in_progress -> review -> closed");
    println!("  A subscriber watches state transitions in real time.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("jira")
            .add_rule_yaml(TICKET_RULE)
            .add_state_machine(ticket_state_machine())
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    let ticket_id = "TICK-42";

    // Each dispatch with the same fingerprint (action_type + ticket_id) advances the state.
    // The state machine auto-assigns the next valid transition based on current state.
    // We send repeated actions to drive the ticket through its lifecycle.

    let transitions = [
        ("new -> open", "open"),
        ("open -> in_progress", "in_progress"),
        ("in_progress -> review", "review"),
        ("review -> closed", "closed"),
    ];

    let mut observed_states = Vec::new();

    for (label, expected_new_state) in &transitions {
        let action = Action::new(
            "engineering",
            "tenant-1",
            "jira",
            "ticket",
            serde_json::json!({
                "ticket_id": ticket_id,
                "target_state": expected_new_state,
            }),
        );

        let outcome = harness.dispatch(&action).await?;

        match &outcome {
            ActionOutcome::StateChanged {
                fingerprint,
                previous_state,
                new_state,
                notify,
            } => {
                observed_states.push(new_state.clone());
                println!(
                    "    [{label}] fingerprint={} prev={previous_state} new={new_state} notify={notify}",
                    &fingerprint[..12.min(fingerprint.len())],
                );
            }
            other => {
                println!("    [{label}] unexpected: {other:?}");
            }
        }
    }

    // Drain and examine subscription events.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    println!("\n  Subscription events ({} total):", events.len());
    let mut state_changed_count = 0;
    for (i, event) in events.iter().enumerate() {
        if let StreamEventType::ActionDispatched {
            outcome:
                ActionOutcome::StateChanged {
                    previous_state,
                    new_state,
                    notify,
                    ..
                },
            ..
        } = &event.event_type
        {
            state_changed_count += 1;
            println!(
                "    #{}: [dispatched/state_changed] {previous_state} -> {new_state} (notify={notify})",
                i + 1,
            );
        } else {
            println!(
                "    #{}: [{:>15}]",
                i + 1,
                event_type_label(&event.event_type)
            );
        }
    }

    println!("\n  State progression: {}", observed_states.join(" -> "));
    assert_eq!(
        observed_states,
        vec!["open", "in_progress", "review", "closed"],
        "ticket should progress through all states"
    );
    assert_eq!(
        state_changed_count, 4,
        "should have 4 state change events on stream"
    );

    harness.teardown().await?;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Rework cycle — review -> in_progress -> review -> closed
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: REWORK CYCLE (review -> in_progress -> review)");
    println!("------------------------------------------------------------------\n");

    println!("  A ticket reaches review, gets sent back to in_progress,");
    println!("  then returns to review and closes. Subscriber sees the loop.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("jira")
            .add_rule_yaml(TICKET_RULE)
            .add_state_machine(ticket_state_machine())
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    let ticket_id = "TICK-77";

    // Drive to review first.
    let advance_states = ["open", "in_progress", "review"];
    for target in &advance_states {
        let action = Action::new(
            "engineering",
            "tenant-1",
            "jira",
            "ticket",
            serde_json::json!({
                "ticket_id": ticket_id,
                "target_state": target,
            }),
        );
        harness.dispatch(&action).await?;
    }

    println!("  Advanced ticket to 'review' state");

    // Now send it back to in_progress (rework).
    let rework = Action::new(
        "engineering",
        "tenant-1",
        "jira",
        "ticket",
        serde_json::json!({
            "ticket_id": ticket_id,
            "target_state": "in_progress",
        }),
    );
    let outcome = harness.dispatch(&rework).await?;
    println!("  -> Rework: {outcome:?}");

    // Return to review.
    let re_review = Action::new(
        "engineering",
        "tenant-1",
        "jira",
        "ticket",
        serde_json::json!({
            "ticket_id": ticket_id,
            "target_state": "review",
        }),
    );
    let outcome = harness.dispatch(&re_review).await?;
    println!("  -> Back to review: {outcome:?}");

    // Close.
    let close = Action::new(
        "engineering",
        "tenant-1",
        "jira",
        "ticket",
        serde_json::json!({
            "ticket_id": ticket_id,
            "target_state": "closed",
        }),
    );
    let outcome = harness.dispatch(&close).await?;
    println!("  -> Closed: {outcome:?}");

    // Check all subscription events.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    println!("\n  Full subscription event log ({} events):", events.len());
    let mut state_transitions = Vec::new();
    for (i, event) in events.iter().enumerate() {
        if let StreamEventType::ActionDispatched {
            outcome:
                ActionOutcome::StateChanged {
                    previous_state,
                    new_state,
                    ..
                },
            ..
        } = &event.event_type
        {
            state_transitions.push(format!("{previous_state}->{new_state}"));
            println!("    #{}: {previous_state} -> {new_state}", i + 1);
        }
    }

    println!("\n  State transitions: [{}]", state_transitions.join(", "));

    // Should see: new->open, open->in_progress, in_progress->review,
    //             review->in_progress, in_progress->review, review->closed
    assert_eq!(
        state_transitions.len(),
        6,
        "should have 6 transitions (including rework loop)"
    );

    harness.teardown().await?;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Multiple tickets with independent state
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 3: MULTIPLE TICKETS (independent state tracking)");
    println!("------------------------------------------------------------------\n");

    println!("  Two tickets progress independently. A subscriber can filter");
    println!("  events by fingerprint to track each ticket separately.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("jira")
            .add_rule_yaml(TICKET_RULE)
            .add_state_machine(ticket_state_machine())
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Ticket A: advance to in_progress.
    for target in &["open", "in_progress"] {
        harness
            .dispatch(&Action::new(
                "engineering",
                "tenant-1",
                "jira",
                "ticket",
                serde_json::json!({"ticket_id": "TICK-A", "target_state": target}),
            ))
            .await?;
    }

    // Ticket B: advance to review.
    for target in &["open", "in_progress", "review"] {
        harness
            .dispatch(&Action::new(
                "engineering",
                "tenant-1",
                "jira",
                "ticket",
                serde_json::json!({"ticket_id": "TICK-B", "target_state": target}),
            ))
            .await?;
    }

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    // Count transitions per ticket by examining the fingerprint.
    let mut transitions_a = 0;
    let mut transitions_b = 0;

    for event in &events {
        if let StreamEventType::ActionDispatched {
            outcome: ActionOutcome::StateChanged { fingerprint, .. },
            ..
        } = &event.event_type
        {
            if fingerprint.contains("TICK-A") {
                transitions_a += 1;
            } else if fingerprint.contains("TICK-B") {
                transitions_b += 1;
            }
        }
    }

    println!("  Total events: {}", events.len());
    println!("  Ticket A transitions: {transitions_a}");
    println!("  Ticket B transitions: {transitions_b}");

    assert_eq!(transitions_a, 2, "TICK-A: new->open, open->in_progress");
    assert_eq!(
        transitions_b, 3,
        "TICK-B: new->open, open->in_progress, in_progress->review"
    );

    harness.teardown().await?;
    println!("\n  [Scenario 3 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("==================================================================");
    println!("              ALL SCENARIOS PASSED");
    println!("==================================================================");

    Ok(())
}
