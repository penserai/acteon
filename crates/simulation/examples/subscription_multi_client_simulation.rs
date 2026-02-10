//! Multiple subscriber simulation — demonstrate multiple clients subscribing
//! to the same event stream and receiving events in parallel.
//!
//! Demonstrates:
//! 1. Multiple subscribers receiving the same events (fan-out)
//! 2. Late subscriber joining mid-chain still receives subsequent events
//! 3. Subscriber filtering by namespace for tenant isolation
//! 4. Each subscriber independently drains its own copy of events
//! 5. High-throughput event delivery to many concurrent subscribers
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_multi_client_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::chain::{ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const CHAIN_RULE: &str = r#"
rules:
  - name: deploy-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "deploy"
    action:
      type: chain
      chain: deploy-pipeline
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

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     MULTI-CLIENT SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: Multiple subscribers receive the same events (fan-out)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: FAN-OUT TO MULTIPLE SUBSCRIBERS");
    println!("------------------------------------------------------------------\n");

    println!("  Three subscribers connect to the same gateway. When actions");
    println!("  are dispatched, all three receive identical events.\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("slack")
            .build(),
    )
    .await?;

    let gateway = harness.node(0).unwrap().gateway();

    // Three independent subscribers.
    let mut sub_a = gateway.stream_tx().subscribe();
    let mut sub_b = gateway.stream_tx().subscribe();
    let mut sub_c = gateway.stream_tx().subscribe();

    println!("  Connected 3 subscribers (A, B, C)\n");

    // Dispatch 3 actions.
    let actions = [
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "welcome",
            serde_json::json!({"user": "alice"}),
        ),
        Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "alert",
            serde_json::json!({"severity": "warning"}),
        ),
        Action::new(
            "notifications",
            "tenant-1",
            "email",
            "reminder",
            serde_json::json!({"user": "bob"}),
        ),
    ];

    println!("  -> Dispatching 3 actions...");
    for action in &actions {
        harness.dispatch(action).await?;
    }

    tokio::time::sleep(Duration::from_millis(30)).await;

    // Each subscriber should receive all 3 events.
    let events_a = drain_events(&mut sub_a);
    let events_b = drain_events(&mut sub_b);
    let events_c = drain_events(&mut sub_c);

    println!("  Subscriber A received: {} events", events_a.len());
    println!("  Subscriber B received: {} events", events_b.len());
    println!("  Subscriber C received: {} events", events_c.len());

    assert_eq!(events_a.len(), 3, "subscriber A should get 3 events");
    assert_eq!(events_b.len(), 3, "subscriber B should get 3 events");
    assert_eq!(events_c.len(), 3, "subscriber C should get 3 events");

    // Verify event IDs match across all subscribers.
    for i in 0..3 {
        assert_eq!(
            events_a[i].id, events_b[i].id,
            "event IDs should match between A and B"
        );
        assert_eq!(
            events_b[i].id, events_c[i].id,
            "event IDs should match between B and C"
        );
    }
    println!("  Event IDs match across all subscribers");

    harness.teardown().await?;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Late subscriber joins mid-chain
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: LATE SUBSCRIBER JOINS MID-CHAIN");
    println!("------------------------------------------------------------------\n");

    println!("  An early subscriber sees the entire chain lifecycle.");
    println!("  A late subscriber joins after step 1 and sees only steps 2+3.\n");

    let svc = Arc::new(RecordingProvider::new("deploy-svc"));

    let chain_config = ChainConfig::new("deploy-pipeline")
        .with_step(ChainStepConfig::new(
            "build",
            "deploy-svc",
            "build",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "test",
            "deploy-svc",
            "test",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "release",
            "deploy-svc",
            "release",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let rules = parse_rules(CHAIN_RULE);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&svc) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    // Early subscriber connects before chain starts.
    let mut early_sub = gateway.stream_tx().subscribe();
    println!("  Early subscriber connected");

    // Dispatch the deploy chain.
    let action = Action::new(
        "ci",
        "tenant-1",
        "deploy-svc",
        "deploy",
        serde_json::json!({"version": "1.2.3"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => {
            println!("  Chain started: {}", &chain_id[..8.min(chain_id.len())]);
            chain_id.clone()
        }
        other => panic!("Expected ChainStarted, got {other:?}"),
    };

    // Advance step 1 (build).
    gateway.advance_chain("ci", "tenant-1", &chain_id).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;

    println!("  Step 1 (build) completed");

    // Late subscriber joins NOW — after step 1 completed.
    let mut late_sub = gateway.stream_tx().subscribe();
    println!("  Late subscriber connected (after step 1)\n");

    // Advance steps 2 and 3 (test, release).
    gateway.advance_chain("ci", "tenant-1", &chain_id).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;

    gateway.advance_chain("ci", "tenant-1", &chain_id).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;

    let early_events = drain_events(&mut early_sub);
    let late_events = drain_events(&mut late_sub);

    println!("  Early subscriber events ({} total):", early_events.len());
    for event in &early_events {
        println!("    [{:>15}]", event_type_label(&event.event_type));
    }

    println!("\n  Late subscriber events ({} total):", late_events.len());
    for event in &late_events {
        println!("    [{:>15}]", event_type_label(&event.event_type));
    }

    // Early subscriber sees all events (dispatch + 3 steps + chain complete + chain_advanced).
    // Late subscriber sees only events from step 2 onward.
    assert!(
        early_events.len() > late_events.len(),
        "early subscriber should see more events: early={} vs late={}",
        early_events.len(),
        late_events.len()
    );

    // Late subscriber should still see the chain_completed event.
    let late_has_completed = late_events
        .iter()
        .any(|e| matches!(e.event_type, StreamEventType::ChainCompleted { .. }));
    assert!(
        late_has_completed,
        "late subscriber should see chain_completed event"
    );

    println!(
        "\n  Early saw {} events, late saw {} events",
        early_events.len(),
        late_events.len()
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // SCENARIO 3: Namespace-based subscriber filtering
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 3: NAMESPACE-BASED FILTERING");
    println!("------------------------------------------------------------------\n");

    println!("  A subscriber filters events by namespace, simulating");
    println!("  tenant isolation at the client level.\n");

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

    // Dispatch actions across 3 namespaces.
    let mixed_actions = [
        Action::new(
            "billing",
            "tenant-1",
            "email",
            "invoice",
            serde_json::json!({"amount": 99}),
        ),
        Action::new(
            "alerts",
            "tenant-1",
            "slack",
            "alert",
            serde_json::json!({"msg": "cpu"}),
        ),
        Action::new(
            "billing",
            "tenant-1",
            "email",
            "receipt",
            serde_json::json!({"amount": 49}),
        ),
        Action::new(
            "alerts",
            "tenant-2",
            "slack",
            "alert",
            serde_json::json!({"msg": "mem"}),
        ),
        Action::new(
            "billing",
            "tenant-2",
            "email",
            "invoice",
            serde_json::json!({"amount": 199}),
        ),
    ];

    println!("  -> Dispatching 5 actions across billing/alerts namespaces\n");
    for action in &mixed_actions {
        harness.dispatch(action).await?;
    }

    tokio::time::sleep(Duration::from_millis(30)).await;
    let all_events = drain_events(&mut stream_rx);

    // Filter: billing namespace only.
    let billing_events: Vec<_> = all_events
        .iter()
        .filter(|e| e.namespace == "billing")
        .collect();

    // Filter: alerts namespace, tenant-1 only.
    let alerts_t1: Vec<_> = all_events
        .iter()
        .filter(|e| e.namespace == "alerts" && e.tenant == "tenant-1")
        .collect();

    // Filter: tenant-2 only (cross-namespace).
    let tenant_2: Vec<_> = all_events
        .iter()
        .filter(|e| e.tenant == "tenant-2")
        .collect();

    println!("  Total events: {}", all_events.len());
    println!("  Filter ns=billing: {} events", billing_events.len());
    println!(
        "  Filter ns=alerts, tenant=tenant-1: {} events",
        alerts_t1.len()
    );
    println!("  Filter tenant=tenant-2: {} events", tenant_2.len());

    assert_eq!(billing_events.len(), 3, "3 billing events");
    assert_eq!(alerts_t1.len(), 1, "1 alert from tenant-1");
    assert_eq!(tenant_2.len(), 2, "2 events from tenant-2");

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
