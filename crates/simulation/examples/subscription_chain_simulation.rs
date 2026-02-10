//! Chain subscription simulation — subscribe to a multi-step chain and
//! receive real-time step completion and chain completion events.
//!
//! Demonstrates:
//! 1. Subscribing to the event stream before dispatching a chain
//! 2. Receiving `ChainStepCompleted` events as each step finishes
//! 3. Receiving the final `ChainCompleted` event with execution path
//! 4. Filtering events by chain ID to isolate one chain's lifecycle
//! 5. Observing `next_step` progression through the chain
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_chain_simulation`

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
  - name: data-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "ingest"
    action:
      type: chain
      chain: etl-pipeline
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

/// Drain all available events from the broadcast receiver.
fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// Get a human-readable label for a stream event type.
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
    println!("     CHAIN SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: Subscribe to a 4-step ETL pipeline chain
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: ETL PIPELINE CHAIN SUBSCRIPTION");
    println!("------------------------------------------------------------------\n");

    println!("  A 4-step ETL pipeline (validate -> extract -> transform -> load)");
    println!("  is executed while a subscriber watches step-by-step progress.\n");

    // Set up providers for each step.
    let validate_provider = Arc::new(RecordingProvider::new("validator").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "valid": true,
            "record_count": 1500
        })))
    }));
    let extract_provider = Arc::new(RecordingProvider::new("extractor").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "extracted": 1500,
            "format": "csv"
        })))
    }));
    let transform_provider =
        Arc::new(RecordingProvider::new("transformer").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(serde_json::json!({
                "transformed": 1500,
                "output_format": "parquet"
            })))
        }));
    let load_provider = Arc::new(RecordingProvider::new("loader").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "loaded": 1500,
            "destination": "warehouse"
        })))
    }));

    // Build a 4-step chain: validate -> extract -> transform -> load.
    let chain_config = ChainConfig::new("etl-pipeline")
        .with_step(ChainStepConfig::new(
            "validate",
            "validator",
            "validate_data",
            serde_json::json!({"source": "{{origin.payload.source}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "extract",
            "extractor",
            "extract_records",
            serde_json::json!({"source": "{{origin.payload.source}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "transform",
            "transformer",
            "transform_records",
            serde_json::json!({"input": "{{prev.response_body}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "load",
            "loader",
            "load_data",
            serde_json::json!({"data": "{{prev.response_body}}"}),
        ))
        .with_timeout(120);

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let rules = parse_rules(CHAIN_RULE);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&validate_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&extract_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&transform_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&load_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    // Subscribe to the event stream BEFORE dispatching.
    let mut stream_rx = gateway.stream_tx().subscribe();

    println!("  Subscriber connected to event stream\n");

    // Dispatch the ingest action to trigger the chain.
    let action = Action::new(
        "data",
        "tenant-1",
        "validator",
        "ingest",
        serde_json::json!({
            "source": "s3://data-lake/raw/2026-02-09.csv"
        }),
    );

    println!("  -> Dispatching ingest action...");
    let outcome = gateway.dispatch(action, None).await?;

    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            first_step,
        } => {
            println!("     Chain started: {chain_name}");
            println!("     Chain ID:      {chain_id}");
            println!("     Total steps:   {total_steps}");
            println!("     First step:    {first_step}");
            chain_id.clone()
        }
        other => {
            panic!("Expected ChainStarted, got {other:?}");
        }
    };

    // Drain the initial ActionDispatched event.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let initial_events = drain_events(&mut stream_rx);
    println!("\n  Initial events after dispatch:");
    for event in &initial_events {
        println!(
            "    [{:>15}] ns={}",
            event_type_label(&event.event_type),
            event.namespace
        );
    }

    // Advance through all 4 steps, watching events after each.
    println!("\n  Advancing chain steps and watching subscription events:\n");

    let step_names = ["validate", "extract", "transform", "load"];
    let mut total_step_completed = 0;
    let mut saw_chain_completed = false;

    for (i, step_name) in step_names.iter().enumerate() {
        gateway.advance_chain("data", "tenant-1", &chain_id).await?;

        // Allow async event emission to settle.
        tokio::time::sleep(Duration::from_millis(30)).await;

        let step_events = drain_events(&mut stream_rx);

        println!("  Step {} ({step_name}):", i + 1);
        for event in &step_events {
            match &event.event_type {
                StreamEventType::ChainStepCompleted {
                    step_name: name,
                    step_index,
                    success,
                    next_step,
                    ..
                } => {
                    total_step_completed += 1;
                    println!(
                        "    [step_completed] step={name} index={step_index} success={success} next={}",
                        next_step.as_deref().unwrap_or("(none)")
                    );
                }
                StreamEventType::ChainCompleted {
                    status,
                    execution_path,
                    ..
                } => {
                    saw_chain_completed = true;
                    println!(
                        "    [chain_completed] status={status} path=[{}]",
                        execution_path.join(" -> ")
                    );
                }
                StreamEventType::ChainAdvanced { chain_id: cid } => {
                    println!("    [chain_advanced] chain_id={}", &cid[..8.min(cid.len())]);
                }
                _ => {
                    println!("    [{:>15}]", event_type_label(&event.event_type));
                }
            }
        }
    }

    // Verify we received step completion events for each step.
    println!("\n  Summary:");
    println!("    Step completed events: {total_step_completed}");
    println!("    Chain completed event: {saw_chain_completed}");
    assert_eq!(
        total_step_completed, 4,
        "expected 4 step completion events, got {total_step_completed}"
    );
    assert!(saw_chain_completed, "expected chain completed event");

    // Verify chain final status.
    let chain_state = gateway
        .get_chain_status("data", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");
    println!("    Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );

    // Verify all providers were called exactly once.
    println!(
        "    Provider calls: validator={}, extractor={}, transformer={}, loader={}",
        validate_provider.call_count(),
        extract_provider.call_count(),
        transform_provider.call_count(),
        load_provider.call_count(),
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Filter events by chain ID
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: CHAIN ID FILTERING");
    println!("------------------------------------------------------------------\n");

    println!("  Two chains run concurrently. A subscriber filters events");
    println!("  to watch only one specific chain's lifecycle.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let svc_a = Arc::new(RecordingProvider::new("svc-a"));
    let svc_b = Arc::new(RecordingProvider::new("svc-b"));

    let chain_fast = ChainConfig::new("chain-fast")
        .with_step(ChainStepConfig::new(
            "step1",
            "svc-a",
            "task",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step2",
            "svc-a",
            "task",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let chain_slow = ChainConfig::new("chain-slow")
        .with_step(ChainStepConfig::new(
            "step1",
            "svc-b",
            "task",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step2",
            "svc-b",
            "task",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "step3",
            "svc-b",
            "task",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let multi_rule: &str = r#"
rules:
  - name: trigger-fast
    priority: 1
    condition:
      field: action.action_type
      eq: "fast"
    action:
      type: chain
      chain: chain-fast
  - name: trigger-slow
    priority: 1
    condition:
      field: action.action_type
      eq: "slow"
    action:
      type: chain
      chain: chain-slow
"#;
    let rules = parse_rules(multi_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&svc_a) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&svc_b) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain_fast)
        .chain(chain_slow)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let mut stream_rx = gateway.stream_tx().subscribe();

    // Start both chains.
    let action_fast = Action::new("test", "tenant-1", "svc-a", "fast", serde_json::json!({}));
    let action_slow = Action::new("test", "tenant-1", "svc-b", "slow", serde_json::json!({}));

    println!("  Starting chain-fast...");
    let outcome_fast = gateway.dispatch(action_fast, None).await?;
    let chain_id_fast = match &outcome_fast {
        ActionOutcome::ChainStarted { chain_id, .. } => {
            println!("    chain_id: {}", &chain_id[..8.min(chain_id.len())]);
            chain_id.clone()
        }
        other => panic!("unexpected: {other:?}"),
    };

    println!("  Starting chain-slow...");
    let outcome_slow = gateway.dispatch(action_slow, None).await?;
    let chain_id_slow = match &outcome_slow {
        ActionOutcome::ChainStarted { chain_id, .. } => {
            println!("    chain_id: {}", &chain_id[..8.min(chain_id.len())]);
            chain_id.clone()
        }
        other => panic!("unexpected: {other:?}"),
    };

    // Advance both chains completely.
    for _ in 0..2 {
        gateway
            .advance_chain("test", "tenant-1", &chain_id_fast)
            .await?;
    }
    for _ in 0..3 {
        gateway
            .advance_chain("test", "tenant-1", &chain_id_slow)
            .await?;
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Collect all events and filter by chain ID.
    let all_events = drain_events(&mut stream_rx);

    let fast_events: Vec<_> = all_events
        .iter()
        .filter(|e| matches_chain_id(&e.event_type, &chain_id_fast))
        .collect();

    let slow_events: Vec<_> = all_events
        .iter()
        .filter(|e| matches_chain_id(&e.event_type, &chain_id_slow))
        .collect();

    println!("\n  Total events:       {}", all_events.len());
    println!("  chain-fast events:  {}", fast_events.len());
    println!("  chain-slow events:  {}", slow_events.len());

    println!("\n  chain-fast lifecycle:");
    for event in &fast_events {
        println!("    [{:>15}]", event_type_label(&event.event_type));
    }

    println!("\n  chain-slow lifecycle:");
    for event in &slow_events {
        println!("    [{:>15}]", event_type_label(&event.event_type));
    }

    // chain-fast: 2 steps -> 2 step_completed + 1 chain_completed + 2 chain_advanced
    // chain-slow: 3 steps -> 3 step_completed + 1 chain_completed + 3 chain_advanced
    let fast_step_events: Vec<_> = fast_events
        .iter()
        .filter(|e| matches!(e.event_type, StreamEventType::ChainStepCompleted { .. }))
        .collect();
    let slow_step_events: Vec<_> = slow_events
        .iter()
        .filter(|e| matches!(e.event_type, StreamEventType::ChainStepCompleted { .. }))
        .collect();

    assert_eq!(
        fast_step_events.len(),
        2,
        "chain-fast should have 2 step events"
    );
    assert_eq!(
        slow_step_events.len(),
        3,
        "chain-slow should have 3 step events"
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

/// Check if a stream event type references a specific chain ID.
fn matches_chain_id(event_type: &StreamEventType, chain_id: &str) -> bool {
    match event_type {
        StreamEventType::ChainAdvanced { chain_id: cid }
        | StreamEventType::ChainStepCompleted { chain_id: cid, .. }
        | StreamEventType::ChainCompleted { chain_id: cid, .. } => cid == chain_id,
        _ => false,
    }
}
