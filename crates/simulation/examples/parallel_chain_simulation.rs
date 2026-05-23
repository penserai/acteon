//! Simulation of parallel chain step (fan-out / fan-in) workflows.
//!
//! Demonstrates:
//! 1. Simple parallel — all sub-steps succeed
//! 2. One failure with fail_fast policy
//! 3. One failure with best_effort policy
//! 4. Any-join — first success wins
//! 5. Parallel group timeout
//! 6. Template resolution from parallel sub-step results
//! 7. Branching after a parallel step
//! 8. Cancel during parallel execution
//!
//! Run with: `cargo run -p acteon-simulation --example parallel_chain_simulation`

use std::sync::Arc;

use acteon_core::chain::{
    BranchCondition, BranchOperator, ChainConfig, ChainStepConfig, ParallelFailurePolicy,
    ParallelJoinPolicy, ParallelStepGroup,
};
use acteon_core::{Action, ActionOutcome, ChainStatus};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tracing::info;

const CHAIN_RULE: &str = r#"
rules:
  - name: trigger-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: test-chain
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("           ACTEON PARALLEL CHAIN STEPS SIMULATION");
    info!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Simple Parallel — All Succeed
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 1: SIMPLE PARALLEL — ALL SUB-STEPS SUCCEED");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let slack_provider = Arc::new(RecordingProvider::new("slack").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"channel": "#alerts", "ts": "123"}),
        ))
    }));
    let email_provider = Arc::new(RecordingProvider::new("email").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"message_id": "msg-456"}),
        ))
    }));
    let pagerduty_provider = Arc::new(RecordingProvider::new("pagerduty").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"incident_id": "INC-789"}),
        ))
    }));

    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_parallel(
            "notify-all",
            ParallelStepGroup {
                steps: vec![
                    ChainStepConfig::new(
                        "slack-alert",
                        "slack",
                        "post_message",
                        serde_json::json!({"text": "Alert fired"}),
                    ),
                    ChainStepConfig::new(
                        "email-alert",
                        "email",
                        "send",
                        serde_json::json!({"subject": "Alert"}),
                    ),
                    ChainStepConfig::new(
                        "page-oncall",
                        "pagerduty",
                        "create_incident",
                        serde_json::json!({"urgency": "high"}),
                    ),
                ],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(30),
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&slack_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&email_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&pagerduty_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "alerts",
        "tenant-1",
        "slack",
        "start_workflow",
        serde_json::json!({"alert_name": "cpu-high"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            ..
        } => {
            info!("  Chain started: {chain_name}");
            chain_id.clone()
        }
        other => panic!("unexpected outcome: {other:?}"),
    };

    // Advance the parallel step — all three sub-steps execute concurrently.
    gateway
        .advance_chain("alerts", "tenant-1", &chain_id)
        .await?;

    let final_state = gateway
        .get_chain_status("alerts", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Completed);
    info!("  All 3 sub-steps executed concurrently");
    info!("    slack calls:     {}", slack_provider.call_count());
    info!("    email calls:     {}", email_provider.call_count());
    info!("    pagerduty calls: {}", pagerduty_provider.call_count());
    info!("  Chain status: {:?}", final_state.status);

    assert_eq!(slack_provider.call_count(), 1);
    assert_eq!(email_provider.call_count(), 1);
    assert_eq!(pagerduty_provider.call_count(), 1);

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 2: One Failure with FailFast Policy
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 2: ONE FAILURE WITH FAIL_FAST POLICY");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let ok_provider = Arc::new(RecordingProvider::new("ok-provider").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"ok": true}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("fail-provider").with_failure_mode(FailureMode::Always));

    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_parallel(
            "parallel-step",
            ParallelStepGroup {
                steps: vec![
                    ChainStepConfig::new("good-step", "ok-provider", "do", serde_json::json!({})),
                    ChainStepConfig::new("bad-step", "fail-provider", "do", serde_json::json!({})),
                ],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(10),
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "ok-provider",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Failed);
    info!("  Parallel group with fail_fast: chain failed due to bad-step");
    info!("  Chain status: {:?}", final_state.status);

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 3: One Failure with BestEffort Policy
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 3: ONE FAILURE WITH BEST_EFFORT POLICY");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let ok_provider = Arc::new(RecordingProvider::new("ok-provider").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"ok": true}),
        ))
    }));
    let fail_provider =
        Arc::new(RecordingProvider::new("fail-provider").with_failure_mode(FailureMode::Always));

    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_parallel(
            "parallel-step",
            ParallelStepGroup {
                steps: vec![
                    ChainStepConfig::new("good-step", "ok-provider", "do", serde_json::json!({})),
                    ChainStepConfig::new("bad-step", "fail-provider", "do", serde_json::json!({})),
                ],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::BestEffort,
                timeout_seconds: Some(10),
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&ok_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&fail_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "ok-provider",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    // With All join + BestEffort, all sub-steps run but one failed → chain fails.
    assert_eq!(final_state.status, ChainStatus::Failed);
    info!("  BestEffort: all sub-steps ran, but chain failed (All join requires all to succeed)");
    info!("  ok-provider calls: {}", ok_provider.call_count());
    info!("  fail-provider calls: {}", fail_provider.call_count());
    info!("  Chain status: {:?}", final_state.status);

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 4: Any-Join — First Success Wins
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 4: ANY-JOIN — FIRST SUCCESS WINS");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let fast_provider = Arc::new(
        RecordingProvider::new("fast-provider").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"source": "fast"}),
            ))
        }),
    );
    let slow_provider = Arc::new(
        RecordingProvider::new("slow-provider").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"source": "slow"}),
            ))
        }),
    );

    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_parallel(
            "race",
            ParallelStepGroup {
                steps: vec![
                    ChainStepConfig::new(
                        "fast-lookup",
                        "fast-provider",
                        "lookup",
                        serde_json::json!({}),
                    ),
                    ChainStepConfig::new(
                        "slow-lookup",
                        "slow-provider",
                        "lookup",
                        serde_json::json!({}),
                    ),
                ],
                join: ParallelJoinPolicy::Any,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(10),
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&fast_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&slow_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "fast-provider",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Completed);
    info!("  Any-join: chain completed on first success");
    info!("  Chain status: {:?}", final_state.status);

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 5: Parallel Group Timeout
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 5: PARALLEL GROUP TIMEOUT");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let slow_provider = Arc::new(
        RecordingProvider::new("slow-provider")
            .with_delay(std::time::Duration::from_secs(10))
            .with_response_fn(|_| {
                Ok(acteon_core::ProviderResponse::success(
                    serde_json::json!({"ok": true}),
                ))
            }),
    );

    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_parallel(
            "timeout-group",
            ParallelStepGroup {
                steps: vec![ChainStepConfig::new(
                    "slow-step",
                    "slow-provider",
                    "do",
                    serde_json::json!({}),
                )],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(1), // 1 second timeout
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::clone(&slow_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
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

    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Failed);
    info!("  Parallel group timed out after 1s");
    info!("  Chain status: {:?}", final_state.status);

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 6: Template Resolution from Parallel Results
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 6: TEMPLATE RESOLUTION FROM PARALLEL RESULTS");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let enrich_provider = Arc::new(RecordingProvider::new("enrichment").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"risk_score": 85, "region": "us-east-1"}),
        ))
    }));
    let lookup_provider = Arc::new(RecordingProvider::new("lookup").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"owner": "team-infra", "tier": "critical"}),
        ))
    }));
    let notify_provider = Arc::new(RecordingProvider::new("notify").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"sent": true}),
        ))
    }));

    let template_rule = r#"
rules:
  - name: trigger-template-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: template-chain
"#;

    let chain = ChainConfig::new("template-chain")
        .with_step(ChainStepConfig::new_parallel(
            "gather-data",
            ParallelStepGroup {
                steps: vec![
                    ChainStepConfig::new(
                        "enrich",
                        "enrichment",
                        "get_risk",
                        serde_json::json!({}),
                    ),
                    ChainStepConfig::new(
                        "lookup-owner",
                        "lookup",
                        "get_owner",
                        serde_json::json!({}),
                    ),
                ],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(10),
                max_concurrency: None,
            },
        ))
        .with_step(ChainStepConfig::new(
            "send-alert",
            "notify",
            "post",
            // Templates referencing the parallel sub-step results by name.
            serde_json::json!({
                "message": "Risk={{steps.enrich.body.risk_score}} Owner={{steps.lookup-owner.body.owner}}"
            }),
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(template_rule))
        .provider(Arc::clone(&enrich_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&lookup_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&notify_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "enrichment",
        "start_workflow",
        serde_json::json!({"host": "web-01"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Step 0: parallel gather-data
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    info!("  Parallel gather-data: enrich + lookup-owner completed");

    // Step 1: send-alert (uses templates from parallel results)
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    info!("  send-alert: dispatched with resolved templates");

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Completed);

    // Verify the notify provider received the resolved payload.
    let last_call = notify_provider.last_action().expect("notify was called");
    let payload_str = last_call.payload.to_string();
    info!("  Resolved payload: {}", last_call.payload);
    assert!(
        payload_str.contains("85"),
        "should contain risk_score from enrich step"
    );
    assert!(
        payload_str.contains("team-infra"),
        "should contain owner from lookup step"
    );

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 7: Branching After Parallel Step
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 7: BRANCHING AFTER PARALLEL STEP");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let check_provider = Arc::new(RecordingProvider::new("checker").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"severity": "critical"}),
        ))
    }));
    let escalate_provider = Arc::new(RecordingProvider::new("escalation").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"escalated": true}),
        ))
    }));
    let log_provider = Arc::new(RecordingProvider::new("logger").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"logged": true}),
        ))
    }));

    let branch_rule = r#"
rules:
  - name: trigger-branch-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: branch-chain
"#;

    // The chain uses branching after a parallel step. Both "escalate" and
    // "just-log" are terminal steps (no further steps after them on their
    // respective paths). The parallel step branches to "escalate" when
    // severity is critical, otherwise falls through to "just-log".
    let chain = ChainConfig::new("branch-chain")
        .with_step(
            ChainStepConfig::new_parallel(
                "gather-checks",
                ParallelStepGroup {
                    steps: vec![ChainStepConfig::new(
                        "severity-check",
                        "checker",
                        "check",
                        serde_json::json!({}),
                    )],
                    join: ParallelJoinPolicy::All,
                    on_failure: ParallelFailurePolicy::FailFast,
                    timeout_seconds: Some(10),
                    max_concurrency: None,
                },
            )
            // Branch on the merged parallel result body.
            .with_branch(BranchCondition::new(
                "body.severity-check.severity",
                BranchOperator::Eq,
                Some(serde_json::json!("critical")),
                "escalate",
            ))
            .with_default_next("just-log"),
        )
        // "just-log" comes first so "escalate" is the terminal step
        // when the branch is taken.
        .with_step(ChainStepConfig::new(
            "just-log",
            "logger",
            "log",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "escalate",
            "escalation",
            "page",
            serde_json::json!({}),
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(branch_rule))
        .provider(Arc::clone(&check_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&escalate_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&log_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "checker",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Step 0: parallel gather-checks → branches to "escalate" because severity == critical
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    info!("  Parallel gather-checks completed, branching on severity...");

    // Step: escalate (branched to)
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Completed);
    assert!(
        escalate_provider.call_count() > 0,
        "escalation should have been reached"
    );
    // The logger should NOT have been called (branch skipped "just-log").
    assert_eq!(
        log_provider.call_count(),
        0,
        "just-log should not be reached"
    );
    info!("  Branched to 'escalate' (severity was critical)");
    info!("  escalation calls: {}", escalate_provider.call_count());
    info!("  logger calls: {} (skipped)", log_provider.call_count());

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    // =========================================================================
    // DEMO 8: Cancel During Parallel Execution
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 8: CANCEL CHAIN WITH PARALLEL STEP");
    info!("------------------------------------------------------------------\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());

    let step_provider = Arc::new(
        RecordingProvider::new("step-provider").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({"ok": true}),
            ))
        }),
    );
    let unreachable_provider = Arc::new(RecordingProvider::new("unreachable"));

    let cancel_rule = r#"
rules:
  - name: trigger-cancel-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "start_workflow"
    action:
      type: chain
      chain: cancel-chain
"#;

    let chain = ChainConfig::new("cancel-chain")
        .with_step(ChainStepConfig::new(
            "setup",
            "step-provider",
            "init",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new_parallel(
            "parallel-work",
            ParallelStepGroup {
                steps: vec![ChainStepConfig::new(
                    "work-a",
                    "unreachable",
                    "do",
                    serde_json::json!({}),
                )],
                join: ParallelJoinPolicy::All,
                on_failure: ParallelFailurePolicy::FailFast,
                timeout_seconds: Some(10),
                max_concurrency: None,
            },
        ))
        .with_timeout(60);

    let gateway = GatewayBuilder::new()
        .state(Arc::clone(&state))
        .lock(Arc::clone(&lock))
        .rules(parse_rules(cancel_rule))
        .provider(Arc::clone(&step_provider) as Arc<dyn acteon_provider::DynProvider>)
        .provider(Arc::clone(&unreachable_provider) as Arc<dyn acteon_provider::DynProvider>)
        .chain(chain)
        .build()?;

    let action = Action::new(
        "test",
        "tenant-1",
        "step-provider",
        "start_workflow",
        serde_json::json!({}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    let chain_id = match &outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id.clone(),
        other => panic!("unexpected: {other:?}"),
    };

    // Advance step 0 (setup)
    gateway.advance_chain("test", "tenant-1", &chain_id).await?;
    info!("  Step 0 (setup): OK");

    // Cancel before the parallel step runs
    gateway
        .cancel_chain(
            "test",
            "tenant-1",
            &chain_id,
            Some("testing cancellation".into()),
            Some("test-runner".into()),
        )
        .await?;

    let final_state = gateway
        .get_chain_status("test", "tenant-1", &chain_id)
        .await?
        .expect("chain state");
    assert_eq!(final_state.status, ChainStatus::Cancelled);
    unreachable_provider.assert_not_called();
    info!("  Chain cancelled before parallel step executed");
    info!("  Chain status: {:?}", final_state.status);
    info!(
        "  unreachable calls: {} (never reached)",
        unreachable_provider.call_count()
    );

    gateway.shutdown().await;
    info!("\n  PASSED\n");

    info!("==================================================================");
    info!("  ALL PARALLEL CHAIN STEPS SIMULATION DEMOS PASSED");
    info!("==================================================================\n");

    Ok(())
}
