//! Branching chain subscription simulation — subscribe to a chain that uses
//! conditional branching and observe real-time branch decisions.
//!
//! Demonstrates:
//! 1. Subscribing to a chain with conditional branches
//! 2. Seeing `ChainStepCompleted` events with `next_step` reflecting branch decisions
//! 3. `ChainCompleted` event with `execution_path` showing the actual branch taken
//! 4. Two different payloads taking different branches through the same chain config
//! 5. Subscribers see only the actual execution path, not all possible paths
//!
//! Run with: `cargo run -p acteon-simulation --example subscription_branching_chain_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::chain::{BranchCondition, BranchOperator, ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome, StreamEvent, StreamEventType};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const RULE_YAML: &str = r#"
rules:
  - name: order-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "process_order"
    action:
      type: chain
      chain: order-fulfillment
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

/// Build a gateway with the order-fulfillment chain that branches based on
/// the `check-inventory` step's response.
///
/// Chain shape:
///   check-inventory -- `in_stock`=="true"  --> ship-standard
///                   -- default             --> backorder
///   ship-standard --> confirm
///   backorder     --> confirm
///   confirm       --> (end)
#[allow(clippy::type_complexity)]
fn build_branching_gateway(
    inventory_response: serde_json::Value,
) -> Result<(acteon_gateway::Gateway, Vec<Arc<RecordingProvider>>), Box<dyn std::error::Error>> {
    let inventory_provider = Arc::new(RecordingProvider::new("inventory-svc").with_response_fn(
        move |_| {
            Ok(acteon_core::ProviderResponse::success(
                inventory_response.clone(),
            ))
        },
    ));
    let shipping_provider = Arc::new(RecordingProvider::new("shipping-svc"));
    let backorder_provider = Arc::new(RecordingProvider::new("backorder-svc"));
    let confirm_provider = Arc::new(RecordingProvider::new("confirm-svc"));

    // Build the branching chain.
    let chain_config = ChainConfig::new("order-fulfillment")
        .with_step(
            ChainStepConfig::new(
                "check-inventory",
                "inventory-svc",
                "check_stock",
                serde_json::json!({"sku": "{{origin.payload.sku}}"}),
            )
            .with_branch(BranchCondition {
                field: "response_body.in_stock".to_string(),
                operator: BranchOperator::Eq,
                value: Some(serde_json::json!("true")),
                target: "ship-standard".to_string(),
            })
            .with_default_next("backorder"),
        )
        .with_step(ChainStepConfig::new(
            "ship-standard",
            "shipping-svc",
            "create_shipment",
            serde_json::json!({"sku": "{{origin.payload.sku}}", "address": "{{origin.payload.address}}"}),
        ).with_default_next("confirm"))
        .with_step(ChainStepConfig::new(
            "backorder",
            "backorder-svc",
            "create_backorder",
            serde_json::json!({"sku": "{{origin.payload.sku}}", "eta": "2 weeks"}),
        ).with_default_next("confirm"))
        .with_step(ChainStepConfig::new(
            "confirm",
            "confirm-svc",
            "send_confirmation",
            serde_json::json!({"email": "{{origin.payload.email}}"}),
        ))
        .with_timeout(60);

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let rules = parse_rules(RULE_YAML);

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .rules(rules)
        .provider(Arc::clone(&inventory_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&shipping_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&backorder_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&confirm_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    Ok((
        gateway,
        vec![
            inventory_provider,
            shipping_provider,
            backorder_provider,
            confirm_provider,
        ],
    ))
}

/// Run a chain to completion, collecting all events along the way.
async fn run_chain_and_collect(
    gateway: &acteon_gateway::Gateway,
    chain_id: &str,
    namespace: &str,
    tenant: &str,
    rx: &mut tokio::sync::broadcast::Receiver<StreamEvent>,
    max_steps: usize,
) -> Vec<StreamEvent> {
    let mut all_events = Vec::new();

    for _ in 0..max_steps {
        let status = gateway
            .get_chain_status(namespace, tenant, chain_id)
            .await
            .ok()
            .flatten();

        if let Some(ref s) = status
            && s.status != acteon_core::chain::ChainStatus::Running
        {
            break;
        }

        let _ = gateway.advance_chain(namespace, tenant, chain_id).await;
        tokio::time::sleep(Duration::from_millis(30)).await;

        all_events.extend(drain_events(rx));
    }

    all_events
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     BRANCHING CHAIN SUBSCRIPTION SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // SCENARIO 1: In-stock order takes the ship-standard branch
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: IN-STOCK ORDER (ship-standard branch)");
    println!("------------------------------------------------------------------\n");

    println!("  An order for an in-stock item routes through:");
    println!("    check-inventory -> ship-standard -> confirm\n");

    let (gateway, providers) = build_branching_gateway(serde_json::json!({
        "in_stock": "true",
        "quantity": 42
    }))?;

    let mut stream_rx = gateway.stream_tx().subscribe();

    let action = Action::new(
        "orders",
        "tenant-1",
        "inventory-svc",
        "process_order",
        serde_json::json!({
            "sku": "WIDGET-001",
            "address": "123 Main St",
            "email": "buyer@example.com"
        }),
    );

    println!("  -> Dispatching in-stock order...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            ..
        } => {
            println!(
                "     Chain: {chain_name}, ID: {}",
                &chain_id[..8.min(chain_id.len())]
            );
            chain_id.clone()
        }
        other => panic!("Expected ChainStarted, got {other:?}"),
    };

    // Drain dispatch event.
    tokio::time::sleep(Duration::from_millis(20)).await;
    drain_events(&mut stream_rx);

    println!("\n  Subscription events as chain executes:\n");
    let events =
        run_chain_and_collect(&gateway, &chain_id, "orders", "tenant-1", &mut stream_rx, 5).await;

    let mut execution_path = Vec::new();
    for event in &events {
        match &event.event_type {
            StreamEventType::ChainStepCompleted {
                step_name,
                step_index,
                success,
                next_step,
                ..
            } => {
                execution_path.push(step_name.clone());
                println!(
                    "    [step_completed] step={step_name} index={step_index} success={success} next={}",
                    next_step.as_deref().unwrap_or("(none)")
                );
            }
            StreamEventType::ChainCompleted {
                status,
                execution_path: path,
                ..
            } => {
                println!(
                    "    [chain_completed] status={status} path=[{}]",
                    path.join(" -> ")
                );
            }
            _ => {
                println!("    [{:>15}]", event_type_label(&event.event_type));
            }
        }
    }

    // Verify the execution path: should skip backorder entirely.
    println!(
        "\n  Observed execution path: [{}]",
        execution_path.join(" -> ")
    );
    assert!(
        execution_path.contains(&"ship-standard".to_string()),
        "in-stock order should take ship-standard branch"
    );
    assert!(
        !execution_path.contains(&"backorder".to_string()),
        "in-stock order should NOT take backorder branch"
    );

    // Verify provider calls.
    println!(
        "  Provider calls: inventory={}, shipping={}, backorder={}, confirm={}",
        providers[0].call_count(),
        providers[1].call_count(),
        providers[2].call_count(),
        providers[3].call_count(),
    );
    assert_eq!(
        providers[1].call_count(),
        1,
        "shipping-svc should be called"
    );
    assert_eq!(
        providers[2].call_count(),
        0,
        "backorder-svc should NOT be called"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Out-of-stock order takes the backorder branch
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: OUT-OF-STOCK ORDER (backorder branch)");
    println!("------------------------------------------------------------------\n");

    println!("  An order for an out-of-stock item routes through:");
    println!("    check-inventory -> backorder -> confirm\n");

    let (gateway, providers) = build_branching_gateway(serde_json::json!({
        "in_stock": "false",
        "quantity": 0
    }))?;

    let mut stream_rx = gateway.stream_tx().subscribe();

    let action = Action::new(
        "orders",
        "tenant-1",
        "inventory-svc",
        "process_order",
        serde_json::json!({
            "sku": "GADGET-999",
            "address": "456 Oak Ave",
            "email": "buyer2@example.com"
        }),
    );

    println!("  -> Dispatching out-of-stock order...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            ..
        } => {
            println!(
                "     Chain: {chain_name}, ID: {}",
                &chain_id[..8.min(chain_id.len())]
            );
            chain_id.clone()
        }
        other => panic!("Expected ChainStarted, got {other:?}"),
    };

    tokio::time::sleep(Duration::from_millis(20)).await;
    drain_events(&mut stream_rx);

    println!("\n  Subscription events as chain executes:\n");
    let events =
        run_chain_and_collect(&gateway, &chain_id, "orders", "tenant-1", &mut stream_rx, 5).await;

    let mut execution_path = Vec::new();
    for event in &events {
        match &event.event_type {
            StreamEventType::ChainStepCompleted {
                step_name,
                next_step,
                ..
            } => {
                execution_path.push(step_name.clone());
                println!(
                    "    [step_completed] step={step_name} next={}",
                    next_step.as_deref().unwrap_or("(none)")
                );
            }
            StreamEventType::ChainCompleted {
                status,
                execution_path: path,
                ..
            } => {
                println!(
                    "    [chain_completed] status={status} path=[{}]",
                    path.join(" -> ")
                );
            }
            _ => {
                println!("    [{:>15}]", event_type_label(&event.event_type));
            }
        }
    }

    println!(
        "\n  Observed execution path: [{}]",
        execution_path.join(" -> ")
    );
    assert!(
        execution_path.contains(&"backorder".to_string()),
        "out-of-stock order should take backorder branch"
    );
    assert!(
        !execution_path.contains(&"ship-standard".to_string()),
        "out-of-stock order should NOT take ship-standard branch"
    );

    println!(
        "  Provider calls: inventory={}, shipping={}, backorder={}, confirm={}",
        providers[0].call_count(),
        providers[1].call_count(),
        providers[2].call_count(),
        providers[3].call_count(),
    );
    assert_eq!(
        providers[1].call_count(),
        0,
        "shipping-svc should NOT be called"
    );
    assert_eq!(
        providers[2].call_count(),
        1,
        "backorder-svc should be called"
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
