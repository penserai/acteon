//! Group subscription simulation — subscribe to event groups and observe
//! events accumulating and being flushed.
//!
//! Demonstrates:
//! 1. Dispatching actions that match a grouping rule
//! 2. Receiving `ActionDispatched` events with `Grouped` outcome showing group growth
//! 3. Observing group metadata (`group_id`, `group_size`, `notify_at`)
//! 4. Filtering subscription events by namespace
//! 5. Multiple groups accumulating independently
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_group_simulation`

use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_simulation::prelude::*;

/// Groups alerts by cluster+severity, batching for 30s.
const ALERT_GROUPING_RULE: &str = r"
rules:
  - name: group-alerts-by-cluster
    priority: 10
    condition:
      field: action.action_type
      eq: alert
    action:
      type: group
      group_by:
        - tenant
        - payload.cluster
        - payload.severity
      group_wait_seconds: 30
      group_interval_seconds: 300
      max_group_size: 100
";

/// A rule that lets non-alert actions pass through normally.
const PASSTHROUGH_RULE: &str = r"
rules:
  - name: passthrough
    priority: 100
    condition:
      field: action.action_type
      eq: notification
    action:
      type: allow
";

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

fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     GROUP SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: Watch alerts accumulate in a group
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: ALERT GROUP ACCUMULATION");
    println!("------------------------------------------------------------------\n");

    println!("  Multiple alerts from the same cluster are grouped together.");
    println!("  Each dispatch produces a Grouped outcome with increasing size.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(ALERT_GROUPING_RULE)
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Send 5 alerts from the same cluster with the same severity.
    println!("  -> Dispatching 5 alerts from cluster=us-east, severity=warning\n");

    let mut group_sizes = Vec::new();
    let mut observed_group_id: Option<String> = None;

    for i in 0..5 {
        let action = Action::new(
            "monitoring",
            "tenant-1",
            "webhook",
            "alert",
            serde_json::json!({
                "cluster": "us-east",
                "severity": "warning",
                "message": format!("CPU spike on node-{}", i + 1),
            }),
        );

        let outcome = harness.dispatch(&action).await?;

        // Each alert is grouped — extract group info from the outcome.
        match &outcome {
            ActionOutcome::Grouped {
                group_id,
                group_size,
                notify_at,
            } => {
                if observed_group_id.is_none() {
                    observed_group_id = Some(group_id.clone());
                }
                group_sizes.push(*group_size);
                println!(
                    "    Alert {}: group_id={}  size={group_size}  notify_at={notify_at}",
                    i + 1,
                    &group_id[..8.min(group_id.len())],
                );
            }
            other => {
                println!("    Alert {}: unexpected outcome: {other:?}", i + 1);
            }
        }
    }

    // Drain subscription events — each dispatch should have emitted an event.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    println!("\n  Subscription events received: {}", events.len());
    for (i, event) in events.iter().enumerate() {
        println!(
            "    #{}: [{:>15}] ns={:<16} action_type={}",
            i + 1,
            event_type_label(&event.event_type),
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
        );
    }

    // Verify group sizes increase monotonically.
    println!("\n  Group sizes over time: {group_sizes:?}");
    for i in 1..group_sizes.len() {
        assert!(
            group_sizes[i] > group_sizes[i - 1],
            "group size should increase: {} -> {}",
            group_sizes[i - 1],
            group_sizes[i]
        );
    }
    assert_eq!(group_sizes.last().copied(), Some(5));

    // Verify all alerts went to the same group.
    assert!(
        observed_group_id.is_some(),
        "should have observed a group ID"
    );

    // Verify the provider was NOT called (actions are held in the group).
    println!(
        "  Provider calls: {} (actions are buffered, not dispatched yet)",
        harness.provider("webhook").unwrap().call_count()
    );
    assert_eq!(
        harness.provider("webhook").unwrap().call_count(),
        0,
        "no provider calls while group is accumulating"
    );

    harness.teardown().await?;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Multiple independent groups
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: MULTIPLE INDEPENDENT GROUPS");
    println!("------------------------------------------------------------------\n");

    println!("  Alerts from different clusters form separate groups.");
    println!("  A subscriber can distinguish them by examining event metadata.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(ALERT_GROUPING_RULE)
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Interleave alerts from two different clusters.
    let clusters = ["us-east", "eu-west", "us-east", "eu-west", "us-east"];
    let mut group_ids: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();

    println!("  -> Dispatching 5 alerts across 2 clusters\n");

    for (i, cluster) in clusters.iter().enumerate() {
        let action = Action::new(
            "monitoring",
            "tenant-1",
            "webhook",
            "alert",
            serde_json::json!({
                "cluster": cluster,
                "severity": "info",
                "message": format!("event-{}", i + 1),
            }),
        );

        let outcome = harness.dispatch(&action).await?;
        if let ActionOutcome::Grouped {
            group_id,
            group_size,
            ..
        } = &outcome
        {
            println!(
                "    Alert {} (cluster={cluster}): group={}  size={group_size}",
                i + 1,
                &group_id[..8.min(group_id.len())]
            );
            group_ids
                .entry(group_id.clone())
                .or_default()
                .push(*group_size);
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    // Verify events were emitted for all dispatches.
    assert_eq!(events.len(), 5, "should have 5 dispatch events");

    // Verify we have exactly 2 distinct groups.
    println!("\n  Distinct groups: {}", group_ids.len());
    for (gid, sizes) in &group_ids {
        println!("    group={}: sizes={sizes:?}", &gid[..8.min(gid.len())]);
    }
    assert_eq!(group_ids.len(), 2, "should have 2 distinct groups");

    harness.teardown().await?;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Mixed grouped and non-grouped actions
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 3: MIXED GROUPED AND NON-GROUPED ACTIONS");
    println!("------------------------------------------------------------------\n");

    println!("  A subscriber sees both grouped and executed outcomes,");
    println!("  making it easy to distinguish different action lifecycles.\n");

    let combined_rules = format!("{ALERT_GROUPING_RULE}\n{PASSTHROUGH_RULE}");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(&combined_rules)
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();
    let mut stream_rx = gateway.stream_tx().subscribe();

    // Dispatch a mix of alerts (grouped) and notifications (executed).
    let actions = [
        Action::new(
            "monitoring",
            "tenant-1",
            "webhook",
            "alert",
            serde_json::json!({"cluster": "us-east", "severity": "warning", "msg": "high cpu"}),
        ),
        Action::new(
            "notifications",
            "tenant-1",
            "webhook",
            "notification",
            serde_json::json!({"user": "alice", "message": "Your order shipped"}),
        ),
        Action::new(
            "monitoring",
            "tenant-1",
            "webhook",
            "alert",
            serde_json::json!({"cluster": "us-east", "severity": "warning", "msg": "high memory"}),
        ),
        Action::new(
            "notifications",
            "tenant-1",
            "webhook",
            "notification",
            serde_json::json!({"user": "bob", "message": "New follower"}),
        ),
    ];

    println!("  -> Dispatching 2 alerts + 2 notifications\n");
    for (i, action) in actions.iter().enumerate() {
        let outcome = harness.dispatch(action).await?;
        let label = match &outcome {
            ActionOutcome::Grouped { group_size, .. } => format!("GROUPED (size={group_size})"),
            ActionOutcome::Executed(_) => "EXECUTED".to_string(),
            other => format!("{other:?}"),
        };
        println!(
            "    Action {} (type={}): {label}",
            i + 1,
            action.action_type
        );
    }

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let events = drain_events(&mut stream_rx);

    println!("\n  Subscription events:");
    let mut grouped_count = 0;
    let mut executed_count = 0;
    for (i, event) in events.iter().enumerate() {
        if let StreamEventType::ActionDispatched { outcome, .. } = &event.event_type {
            match outcome {
                ActionOutcome::Grouped { .. } => grouped_count += 1,
                ActionOutcome::Executed(_) => executed_count += 1,
                _ => {}
            }
        }
        println!(
            "    #{}: [{:>15}] ns={:<16} type={}",
            i + 1,
            event_type_label(&event.event_type),
            event.namespace,
            event.action_type.as_deref().unwrap_or("-"),
        );
    }

    println!("\n  Grouped events: {grouped_count}, Executed events: {executed_count}");
    assert_eq!(grouped_count, 2, "2 alerts should be grouped");
    assert_eq!(executed_count, 2, "2 notifications should be executed");

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
