//! Simulation of sub-chain workflows.
//!
//! Demonstrates:
//! 1. Simple parent → sub-chain → continue
//! 2. Sub-chain failure with Abort policy
//! 3. Sub-chain failure with Skip policy
//! 4. Nested sub-chains (depth 2)
//! 5. Cross-chain cycle detection at build time
//! 6. Cancellation cascade
//! 7. Template resolution through sub-chains
//!
//! Run with: `cargo run -p acteon-simulation --example sub_chain_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::chain::{ChainConfig, ChainStepConfig, StepFailurePolicy};
use acteon_core::{Action, ActionOutcome, ChainStatus};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const CHAIN_RULE: &str = r#"
rules:
  - name: trigger-parent
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("           ACTEON SUB-CHAIN SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Simple Parent -> Sub-Chain -> Continue
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 1: SIMPLE PARENT -> SUB-CHAIN -> CONTINUE");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let check_provider = Arc::new(RecordingProvider::new("monitoring-api").with_response_fn(
        |_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"severity": "high"}),
            ))
        },
    ));
    let pagerduty_provider = Arc::new(RecordingProvider::new("pagerduty").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"incident_id": "INC-42"}),
        ))
    }));
    let slack_provider = Arc::new(RecordingProvider::new("slack").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"message_id": "msg-99"}),
        ))
    }));
    let ticketing_provider = Arc::new(RecordingProvider::new("ticketing").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"closed": true}),
        ))
    }));

    // Parent chain: check -> escalate (sub-chain) -> resolve
    let parent_chain = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new(
            "check-severity",
            "monitoring-api",
            "get_severity",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_sub_chain(
            "escalate",
            "escalate-and-notify",
        ))
        .with_step(ChainStepConfig::new(
            "resolve",
            "ticketing",
            "close_ticket",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    // Sub-chain: page -> notify
    let sub_chain = ChainConfig::new("escalate-and-notify")
        .with_step(ChainStepConfig::new(
            "page-oncall",
            "pagerduty",
            "create_incident",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "notify-channel",
            "slack",
            "post_message",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rules = parse_rules(CHAIN_RULE);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(rules)
        .provider(Arc::clone(&check_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&pagerduty_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&slack_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&ticketing_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(parent_chain)
        .chain(sub_chain)
        .build()?;

    let action = Action::new(
        "incidents",
        "tenant-1",
        "monitoring-api",
        "start_workflow",
        serde_json::json!({"alert": "cpu-high"}),
    );

    println!("  Dispatching action to start parent chain...");
    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            total_steps,
            ..
        } => {
            println!("  Chain started: {chain_name} ({total_steps} steps)");
            println!("    chain_id: {chain_id}");
            chain_id.clone()
        }
        other => {
            println!("  Unexpected outcome: {other:?}");
            return Ok(());
        }
    };

    // Step 0: check-severity (regular provider step)
    gateway
        .advance_chain("incidents", "tenant-1", &chain_id)
        .await?;
    println!("  Step 0 (check-severity): advanced OK");

    // Step 1: escalate (sub-chain) — first advance spawns child, sets WaitingSubChain
    gateway
        .advance_chain("incidents", "tenant-1", &chain_id)
        .await?;
    println!("  Step 1 (escalate): sub-chain spawned, parent waiting");

    let parent_state = gateway
        .get_chain_status("incidents", "tenant-1", &chain_id)
        .await?
        .expect("parent chain state");
    assert_eq!(parent_state.status, ChainStatus::WaitingSubChain);
    assert!(!parent_state.child_chain_ids.is_empty());
    let child_id = &parent_state.child_chain_ids[0];
    println!("    Child chain ID: {child_id}");

    // Advance the child chain: step 0 (page-oncall), step 1 (notify-channel)
    gateway
        .advance_chain("incidents", "tenant-1", child_id)
        .await?;
    println!("  Child step 0 (page-oncall): advanced OK");
    gateway
        .advance_chain("incidents", "tenant-1", child_id)
        .await?;
    println!("  Child step 1 (notify-channel): advanced OK");

    let child_state = gateway
        .get_chain_status("incidents", "tenant-1", child_id)
        .await?
        .expect("child chain state");
    assert_eq!(child_state.status, ChainStatus::Completed);
    assert_eq!(child_state.parent_chain_id.as_deref(), Some(&*chain_id));
    println!("  Child chain completed");

    // Now advance the parent again — it should pick up the child result and continue
    gateway
        .advance_chain("incidents", "tenant-1", &chain_id)
        .await?;
    println!("  Step 1 (escalate): sub-chain result extracted, moving forward");

    // Step 2: resolve (regular provider step)
    gateway
        .advance_chain("incidents", "tenant-1", &chain_id)
        .await?;
    println!("  Step 2 (resolve): advanced OK");

    let final_state = gateway
        .get_chain_status("incidents", "tenant-1", &chain_id)
        .await?
        .expect("final parent chain state");
    assert_eq!(final_state.status, ChainStatus::Completed);
    println!("\n  Final parent chain status: {:?}", final_state.status);

    println!("  Provider call counts:");
    println!("    monitoring-api: {}", check_provider.call_count());
    println!("    pagerduty:      {}", pagerduty_provider.call_count());
    println!("    slack:          {}", slack_provider.call_count());
    println!("    ticketing:      {}", ticketing_provider.call_count());

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 2: Sub-Chain Failure with Abort Policy
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 2: SUB-CHAIN FAILURE (ABORT POLICY)");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let ok_provider = Arc::new(RecordingProvider::new("step-ok").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"ok": true}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("step-fail").with_failure_mode(FailureMode::Always));
    let unreachable = Arc::new(RecordingProvider::new("unreachable"));

    let parent = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "step-ok",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_sub_chain(
            "invoke-child",
            "child-chain",
        ))
        .with_step(ChainStepConfig::new(
            "after-sub",
            "unreachable",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let child = ChainConfig::new("child-chain")
        .with_step(ChainStepConfig::new(
            "failing-step",
            "step-fail",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let abort_rule = r#"
rules:
  - name: trigger-abort-test
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(abort_rule))
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&unreachable) as Arc<dyn acteon_provider::DynProvider>)
        .chain(parent)
        .chain(child)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "step-ok",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected outcome: {other:?}"),
    };

    // Step 0: first (ok)
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 0 (first): OK");

    // Step 1: invoke-child — spawns child
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 1 (invoke-child): sub-chain spawned");

    let parent_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    let child_id = &parent_state.child_chain_ids[0];

    // Advance child — failing step
    gateway.advance_chain("test", "tenant-1", child_id).await?;
    println!("  Child step 0 (failing-step): failed");

    let child_state = gateway
        .get_chain_status("test", "tenant-1", child_id)
        .await?
        .unwrap();
    assert_eq!(child_state.status, ChainStatus::Failed);
    println!("  Child chain status: {:?}", child_state.status);

    // Advance parent — should detect child failure, abort
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    assert_eq!(final_state.status, ChainStatus::Failed);
    println!(
        "  Parent chain status: {:?} (aborted due to sub-chain failure)",
        final_state.status
    );
    unreachable.assert_not_called();

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 3: Sub-Chain Failure with Skip Policy
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 3: SUB-CHAIN FAILURE (SKIP POLICY)");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let ok_provider = Arc::new(RecordingProvider::new("step-ok").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"ok": true}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("step-fail").with_failure_mode(FailureMode::Always));
    let final_provider = Arc::new(RecordingProvider::new("step-final").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"final": true}),
        ))
    }));

    let parent = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "step-ok",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_step(
            ChainStepConfig::new_sub_chain("invoke-child", "child-chain")
                .with_on_failure(StepFailurePolicy::Skip),
        )
        .with_step(ChainStepConfig::new(
            "after-sub",
            "step-final",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let child = ChainConfig::new("child-chain")
        .with_step(ChainStepConfig::new(
            "failing-step",
            "step-fail",
            "do_thing",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let skip_rule = r#"
rules:
  - name: trigger-skip-test
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(skip_rule))
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&final_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(parent)
        .chain(child)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "step-ok",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected outcome: {other:?}"),
    };

    // Step 0
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 0: OK");

    // Step 1 — spawn child
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 1: sub-chain spawned");

    let ps = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    let child_id = &ps.child_chain_ids[0];

    // Advance child — fails
    gateway.advance_chain("test", "tenant-1", child_id).await?;
    println!("  Child: failed");

    // Advance parent — skip policy, should continue
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 1: sub-chain failure skipped");

    // Step 2 — final step
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    println!("  Step 2: OK");

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    assert_eq!(final_state.status, ChainStatus::Completed);
    println!(
        "\n  Parent completed despite sub-chain failure: {:?}",
        final_state.status
    );
    assert!(
        final_provider.call_count() > 0,
        "final step should have been reached"
    );

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 4: Nested Sub-Chains (depth 2)
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 4: NESTED SUB-CHAINS (DEPTH 2)");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let provider_a = Arc::new(RecordingProvider::new("provider-a").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"a": true}),
        ))
    }));
    let provider_b = Arc::new(RecordingProvider::new("provider-b").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"b": true}),
        ))
    }));
    let provider_c = Arc::new(RecordingProvider::new("provider-c").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"c": true}),
        ))
    }));

    let grandparent = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new(
            "step-a",
            "provider-a",
            "do",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_sub_chain("invoke-mid", "mid-chain"))
        .with_timeout(60);

    let mid = ChainConfig::new("mid-chain")
        .with_step(ChainStepConfig::new(
            "step-b",
            "provider-b",
            "do",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_sub_chain("invoke-leaf", "leaf-chain"))
        .with_timeout(60);

    let leaf = ChainConfig::new("leaf-chain")
        .with_step(ChainStepConfig::new(
            "step-c",
            "provider-c",
            "do",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    let nested_rule = r#"
rules:
  - name: trigger-nested
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(nested_rule))
        .provider(Arc::clone(&provider_a) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&provider_b) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&provider_c) as Arc<dyn acteon_provider::DynProvider>)
        .chain(grandparent)
        .chain(mid)
        .chain(leaf)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "provider-a",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let gp_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Grandparent step 0
    gateway.advance_chain("test", "tenant-1", &gp_id).await?;
    println!("  Grandparent step 0: OK");

    // Grandparent step 1 — spawns mid chain
    gateway.advance_chain("test", "tenant-1", &gp_id).await?;
    println!("  Grandparent step 1: sub-chain spawned (mid-chain)");

    let gp_state = gateway
        .get_chain_status("test", "tenant-1", &gp_id)
        .await?
        .unwrap();
    let mid_id = &gp_state.child_chain_ids[0];

    // Mid step 0
    gateway.advance_chain("test", "tenant-1", mid_id).await?;
    println!("  Mid step 0: OK");

    // Mid step 1 — spawns leaf chain
    gateway.advance_chain("test", "tenant-1", mid_id).await?;
    println!("  Mid step 1: sub-chain spawned (leaf-chain)");

    let mid_state = gateway
        .get_chain_status("test", "tenant-1", mid_id)
        .await?
        .unwrap();
    let leaf_id = &mid_state.child_chain_ids[0];

    // Leaf step 0
    gateway.advance_chain("test", "tenant-1", leaf_id).await?;
    println!("  Leaf step 0: OK");

    let leaf_state = gateway
        .get_chain_status("test", "tenant-1", leaf_id)
        .await?
        .unwrap();
    assert_eq!(leaf_state.status, ChainStatus::Completed);
    println!("  Leaf chain completed");

    // Resume mid — picks up leaf result
    gateway.advance_chain("test", "tenant-1", mid_id).await?;
    println!("  Mid: resumed after leaf completion");

    let mid_state = gateway
        .get_chain_status("test", "tenant-1", mid_id)
        .await?
        .unwrap();
    assert_eq!(mid_state.status, ChainStatus::Completed);
    println!("  Mid chain completed");

    // Resume grandparent — picks up mid result
    gateway.advance_chain("test", "tenant-1", &gp_id).await?;
    println!("  Grandparent: resumed after mid completion");

    let gp_state = gateway
        .get_chain_status("test", "tenant-1", &gp_id)
        .await?
        .unwrap();
    assert_eq!(gp_state.status, ChainStatus::Completed);
    println!("\n  All three levels completed: {:?}", gp_state.status);
    println!("  provider-a calls: {}", provider_a.call_count());
    println!("  provider-b calls: {}", provider_b.call_count());
    println!("  provider-c calls: {}", provider_c.call_count());

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 5: Cycle Detection at Build Time
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 5: CROSS-CHAIN CYCLE DETECTION");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let chain_a =
        ChainConfig::new("chain-a").with_step(ChainStepConfig::new_sub_chain("call-b", "chain-b"));
    let chain_b =
        ChainConfig::new("chain-b").with_step(ChainStepConfig::new_sub_chain("call-a", "chain-a"));

    let result = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .chain(chain_a)
        .chain(chain_b)
        .build();

    match result {
        Err(e) => {
            println!("  Build correctly rejected cyclic chains:");
            println!("    Error: {e}");
            assert!(
                e.to_string().contains("cycle"),
                "error should mention cycle"
            );
        }
        Ok(_) => {
            panic!("expected cycle detection error");
        }
    }

    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 6: Cancellation Cascade
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 6: CANCELLATION CASCADE");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let slow_provider = Arc::new(
        RecordingProvider::new("slow-provider").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"ok": true}),
            ))
        }),
    );

    let parent = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "slow-provider",
            "do",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_sub_chain(
            "invoke-child",
            "child-chain",
        ))
        .with_timeout(60);

    let child = ChainConfig::new("child-chain")
        .with_step(ChainStepConfig::new(
            "child-step-1",
            "slow-provider",
            "do",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "child-step-2",
            "slow-provider",
            "do",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    let cancel_rule = r#"
rules:
  - name: trigger-cancel-test
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(cancel_rule))
        .provider(Arc::clone(&slow_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(parent)
        .chain(child)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "slow-provider",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Advance to sub-chain step, spawn child
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let ps = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    let child_id = ps.child_chain_ids[0].clone();

    // Advance child one step (still running)
    gateway.advance_chain("test", "tenant-1", &child_id).await?;
    let cs = gateway
        .get_chain_status("test", "tenant-1", &child_id)
        .await?
        .unwrap();
    assert_eq!(cs.status, ChainStatus::Running);
    println!("  Child chain running (1 of 2 steps done)");

    // Cancel the parent — should cascade to child
    gateway
        .cancel_chain(
            "test",
            "tenant-1",
            &chain_id,
            "simulation test",
            "test-runner",
        )
        .await?;
    println!("  Parent chain cancelled");

    let parent_final = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    assert_eq!(parent_final.status, ChainStatus::Cancelled);
    println!("  Parent status: {:?}", parent_final.status);

    let child_final = gateway
        .get_chain_status("test", "tenant-1", &child_id)
        .await?
        .unwrap();
    assert_eq!(child_final.status, ChainStatus::Cancelled);
    println!(
        "  Child status:  {:?} (cascaded cancellation)",
        child_final.status
    );

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 7: Origin Action Inherited by Sub-Chain
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 7: ORIGIN ACTION INHERITED BY SUB-CHAIN");
    println!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let provider = Arc::new(
        RecordingProvider::new("test-provider").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"ok": true}),
            ))
        }),
    );

    let parent = ChainConfig::new("parent-chain")
        .with_step(ChainStepConfig::new_sub_chain("invoke", "child-chain"))
        .with_timeout(60);

    let child = ChainConfig::new("child-chain")
        .with_step(ChainStepConfig::new(
            "echo",
            "test-provider",
            "do",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    let origin_rule = r#"
rules:
  - name: trigger-origin-test
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: parent-chain
"#;

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(origin_rule))
        .provider(Arc::clone(&provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(parent)
        .chain(child)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "test-provider",
        "start_workflow",
        serde_json::json!({"user": "alice", "request_id": "req-123"}),
    );
    let origin_payload = action.payload.clone();

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Spawn child
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let ps = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .unwrap();
    let child_id = &ps.child_chain_ids[0];

    let child_state = gateway
        .get_chain_status("test", "tenant-1", child_id)
        .await?
        .unwrap();

    // Verify the child inherited the origin action's payload
    assert_eq!(child_state.origin_action.payload, origin_payload);
    println!("  Child chain inherited origin action payload:");
    println!(
        "    {}",
        serde_json::to_string_pretty(&child_state.origin_action.payload)?
    );

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    println!("==================================================================");
    println!("  ALL SUB-CHAIN SIMULATION DEMOS PASSED");
    println!("==================================================================\n");

    Ok(())
}
