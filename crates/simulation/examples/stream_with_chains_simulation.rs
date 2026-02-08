//! Streaming chain progress via SSE event stream.
//!
//! This simulation demonstrates how an SSE subscriber can watch chain
//! execution progress in real time. It shows:
//!
//! 1. Starting a multi-step chain action
//! 2. Receiving `ChainAdvanced` events as each step completes
//! 3. The initial `ActionDispatched` event with `ChainStarted` outcome
//! 4. Observing the entire chain lifecycle through the event stream
//!
//! Run with: `cargo run -p acteon-simulation --example stream_with_chains_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_audit::store::AuditStore;
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::chain::{ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome, StreamEventType};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const CHAIN_RULE: &str = r#"
rules:
  - name: research-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "research"
    action:
      type: chain
      chain: search-summarize-notify
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║      STREAM WITH CHAINS SIMULATION DEMO                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Watch Chain Progress in Real Time
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: CHAIN PROGRESS VIA EVENT STREAM");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A 3-step research chain (search -> summarize -> notify) is");
    println!("  executed while an SSE subscriber watches the progress.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());

    let search_provider = Arc::new(RecordingProvider::new("search-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "results": [
                {"title": "Rust async primer", "url": "https://example.com/1"},
                {"title": "Tokio deep dive", "url": "https://example.com/2"},
            ]
        })))
    }));
    let summarize_provider = Arc::new(RecordingProvider::new("llm-api").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "summary": "Rust async uses Futures with Tokio as the primary runtime."
        })))
    }));
    let notify_provider = Arc::new(RecordingProvider::new("email"));

    let chain_config = ChainConfig::new("search-summarize-notify")
        .with_step(ChainStepConfig::new(
            "search",
            "search-api",
            "web_search",
            serde_json::json!({"query": "{{origin.payload.query}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "summarize",
            "llm-api",
            "summarize",
            serde_json::json!({
                "text": "{{prev.response_body}}",
                "max_length": 200
            }),
        ))
        .with_step(ChainStepConfig::new(
            "notify",
            "email",
            "send_email",
            serde_json::json!({
                "to": "{{origin.payload.email}}",
                "subject": "Research: {{origin.payload.query}}",
                "body": "{{prev.response_body.summary}}"
            }),
        ))
        .with_timeout(60);

    let rules = parse_rules(CHAIN_RULE);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&search_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&summarize_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&notify_provider) as Arc<dyn acteon_provider::DynProvider>)
        .audit(Arc::clone(&audit))
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    // Subscribe to the event stream before starting the chain.
    let mut stream_rx = gateway.stream_tx().subscribe();

    println!("  SSE subscriber connected\n");

    // Dispatch the research action (triggers chain).
    let action = Action::new(
        "research",
        "tenant-1",
        "search-api",
        "research",
        serde_json::json!({
            "query": "rust async programming",
            "email": "researcher@example.com"
        }),
    );

    println!("  -> Dispatching research action...");
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
            println!("     Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Advance through all 3 steps, checking for events after each.
    println!("\n  Advancing chain steps and watching events:\n");

    let step_names = ["search", "summarize", "notify"];
    for (i, step_name) in step_names.iter().enumerate() {
        gateway
            .advance_chain("research", "tenant-1", &chain_id)
            .await?;

        // Allow async event emission to settle.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check for new events.
        let mut step_events = Vec::new();
        while let Ok(event) = stream_rx.try_recv() {
            step_events.push(event);
        }

        println!("  Step {i} ({step_name}): advanced");
        for event in &step_events {
            println!(
                "    Event: [{:>12}] ns={:<12} chain={}",
                event_type_label(&event.event_type),
                event.namespace,
                extract_chain_id(&event.event_type).unwrap_or("-"),
            );
        }
    }

    // Verify provider call counts.
    println!("\n  Provider call counts:");
    println!("    search-api: {}", search_provider.call_count());
    println!("    llm-api:    {}", summarize_provider.call_count());
    println!("    email:      {}", notify_provider.call_count());

    // Verify chain completed.
    let chain_state = gateway
        .get_chain_status("research", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");
    println!("\n  Final chain status: {:?}", chain_state.status);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 1 passed]\n");

    // =========================================================================
    // SCENARIO 2: Multiple Chains with Filtered Monitoring
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: MULTIPLE CHAINS, FILTERED MONITORING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Two chains run concurrently. A subscriber filters events");
    println!("  to watch only one specific chain.\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let svc_a = Arc::new(RecordingProvider::new("svc-a"));
    let svc_b = Arc::new(RecordingProvider::new("svc-b"));

    let chain_alpha = ChainConfig::new("chain-alpha")
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

    let chain_beta = ChainConfig::new("chain-beta")
        .with_step(ChainStepConfig::new(
            "step1",
            "svc-b",
            "task",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let multi_rule: &str = r#"
rules:
  - name: trigger-alpha
    priority: 1
    condition:
      field: action.action_type
      eq: "alpha"
    action:
      type: chain
      chain: chain-alpha
  - name: trigger-beta
    priority: 1
    condition:
      field: action.action_type
      eq: "beta"
    action:
      type: chain
      chain: chain-beta
"#;
    let rules = parse_rules(multi_rule);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&svc_a) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&svc_b) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain_alpha)
        .chain(chain_beta)
        .completed_chain_ttl(Duration::from_secs(3600))
        .build()?;

    let mut stream_rx = gateway.stream_tx().subscribe();

    // Start both chains.
    let action_a = Action::new("test", "tenant-1", "svc-a", "alpha", serde_json::json!({}));
    let action_b = Action::new("test", "tenant-1", "svc-b", "beta", serde_json::json!({}));

    println!("  Starting chain-alpha...");
    let outcome_a = gateway.dispatch(action_a, None).await?;
    let chain_id_a = match &outcome_a {
        ActionOutcome::ChainStarted { chain_id, .. } => {
            println!("    chain_id: {chain_id}");
            chain_id.clone()
        }
        other => panic!("unexpected: {other:?}"),
    };

    println!("  Starting chain-beta...");
    let outcome_b = gateway.dispatch(action_b, None).await?;
    let chain_id_b = match &outcome_b {
        ActionOutcome::ChainStarted { chain_id, .. } => {
            println!("    chain_id: {chain_id}");
            chain_id.clone()
        }
        other => panic!("unexpected: {other:?}"),
    };

    // Advance both chains.
    for _ in 0..2 {
        gateway
            .advance_chain("test", "tenant-1", &chain_id_a)
            .await?;
    }
    gateway
        .advance_chain("test", "tenant-1", &chain_id_b)
        .await?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Collect all events and filter for chain-alpha only.
    let mut all_events = Vec::new();
    while let Ok(event) = stream_rx.try_recv() {
        all_events.push(event);
    }

    let alpha_events: Vec<_> = all_events
        .iter()
        .filter(|e| extract_chain_id(&e.event_type).is_some_and(|id| id == chain_id_a))
        .collect();

    let beta_events: Vec<_> = all_events
        .iter()
        .filter(|e| extract_chain_id(&e.event_type).is_some_and(|id| id == chain_id_b))
        .collect();

    println!("\n  Total events:        {}", all_events.len());
    println!("  Chain-alpha events:  {}", alpha_events.len());
    println!("  Chain-beta events:   {}", beta_events.len());

    for event in &alpha_events {
        println!(
            "    alpha: [{:>12}] chain={}",
            event_type_label(&event.event_type),
            &extract_chain_id(&event.event_type).unwrap_or("-")
                [..8.min(extract_chain_id(&event.event_type).unwrap_or("-").len())],
        );
    }

    gateway.shutdown().await;
    println!("\n  [Scenario 2 passed]\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║              ALL SCENARIOS PASSED                           ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
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
    }
}

/// Extract `chain_id` from a stream event type, if applicable.
fn extract_chain_id(event_type: &StreamEventType) -> Option<&str> {
    match event_type {
        StreamEventType::ChainAdvanced { chain_id } => Some(chain_id),
        _ => None,
    }
}
