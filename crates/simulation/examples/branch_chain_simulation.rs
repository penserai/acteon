//! Simulation of conditional branching in task chains.
//!
//! Demonstrates:
//! 1. Incident severity routing (branch on response body field)
//! 2. Success/failure branching (branch on step success)
//! 3. Multi-step branch paths converging to a common finalize step
//! 4. Default fallthrough when no branch condition matches
//! 5. Linear chain backward compatibility (`execution_path` populated)
//!
//! Run with: `cargo run -p acteon-simulation --example branch_chain_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::chain::{BranchCondition, BranchOperator, ChainConfig, ChainStepConfig};
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

/// Helper: build a gateway with given providers, chain, and rule YAML.
#[allow(clippy::unused_async)]
async fn build_gateway(
    providers: Vec<Arc<dyn acteon_provider::DynProvider>>,
    chain_config: ChainConfig,
    rule_yaml: &str,
) -> Result<acteon_gateway::Gateway, Box<dyn std::error::Error>> {
    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let rules = parse_rules(rule_yaml);

    let mut builder = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .rules(rules)
        .chain(chain_config)
        .completed_chain_ttl(Duration::from_secs(3600));

    for p in providers {
        builder = builder.provider(p);
    }

    Ok(builder.build()?)
}

/// Helper: dispatch an action and extract the `chain_id` from `ChainStarted`.
async fn start_chain(
    gateway: &acteon_gateway::Gateway,
    namespace: &str,
    tenant: &str,
    provider: &str,
    action_type: &str,
    payload: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let action = Action::new(namespace, tenant, provider, action_type, payload);
    let outcome = gateway.dispatch(action, None).await?;
    match outcome {
        ActionOutcome::ChainStarted { chain_id, .. } => Ok(chain_id),
        other => Err(format!("expected ChainStarted, got {other:?}").into()),
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines, clippy::similar_names)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     ACTEON BRANCH CHAIN SIMULATION");
    println!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Incident Severity Routing
    // =========================================================================
    //
    // Chain shape:
    //   check-severity ─┬─ severity=="critical" ──> escalate ──> done
    //                   ├─ severity=="warning"  ──> notify   ──> done
    //                   └─ default              ──> log-only ──> done
    //
    // Each branch target uses with_default_next("done") to skip to the
    // terminal step, preventing sequential fallthrough between branches.
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 1: INCIDENT SEVERITY ROUTING");
    println!("------------------------------------------------------------------\n");

    let check_provider = Arc::new(
        RecordingProvider::new("incident-api").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(serde_json::json!({
                "severity": "critical",
                "source": "monitoring"
            })))
        }),
    );
    let pagerduty_provider = Arc::new(RecordingProvider::new("pagerduty"));
    let slack_provider = Arc::new(RecordingProvider::new("slack"));
    let logger_provider = Arc::new(RecordingProvider::new("logger"));
    let done_provider = Arc::new(RecordingProvider::new("done-svc"));

    // The "done" step is the terminal convergence point. Each exclusive
    // branch target explicitly routes here via default_next.
    let chain_config = ChainConfig::new("incident-routing")
        .with_step(
            ChainStepConfig::new(
                "check-severity",
                "incident-api",
                "check_incident",
                serde_json::json!({"incident_id": "{{origin.payload.incident_id}}"}),
            )
            .with_branch(BranchCondition::new(
                "body.severity",
                BranchOperator::Eq,
                Some(serde_json::json!("critical")),
                "escalate",
            ))
            .with_branch(BranchCondition::new(
                "body.severity",
                BranchOperator::Eq,
                Some(serde_json::json!("warning")),
                "notify",
            ))
            .with_default_next("log-only"),
        )
        .with_step(
            ChainStepConfig::new(
                "escalate",
                "pagerduty",
                "create_alert",
                serde_json::json!({"urgency": "high"}),
            )
            .with_default_next("done"),
        )
        .with_step(
            ChainStepConfig::new(
                "notify",
                "slack",
                "send_message",
                serde_json::json!({"channel": "#incidents"}),
            )
            .with_default_next("done"),
        )
        .with_step(
            ChainStepConfig::new(
                "log-only",
                "logger",
                "log",
                serde_json::json!({"level": "info"}),
            )
            .with_default_next("done"),
        )
        .with_step(ChainStepConfig::new(
            "done",
            "done-svc",
            "ack",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rule_yaml = r#"
rules:
  - name: incident-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "incident"
    action:
      type: chain
      chain: incident-routing
"#;

    let gateway = build_gateway(
        vec![
            Arc::clone(&check_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&pagerduty_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&slack_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&logger_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&done_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config,
        rule_yaml,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "ops",
        "tenant-1",
        "incident-api",
        "incident",
        serde_json::json!({"incident_id": "INC-001"}),
    )
    .await?;
    println!("  Chain started: {chain_id}");

    // Advance check-severity -> branches to "escalate" (severity == critical)
    gateway.advance_chain("ops", "tenant-1", &chain_id).await?;
    println!("  Step 0 (check-severity) advanced -> escalate");

    // Advance escalate -> default_next "done"
    gateway.advance_chain("ops", "tenant-1", &chain_id).await?;
    println!("  Step 1 (escalate) advanced -> done");

    // Advance done -> chain completes
    gateway.advance_chain("ops", "tenant-1", &chain_id).await?;
    println!("  Step 2 (done) advanced -> complete");

    let chain_state = gateway
        .get_chain_status("ops", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("\n  Final chain status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["check-severity", "escalate", "done"]
    );

    println!("\n  Provider call counts:");
    println!("    incident-api: {}", check_provider.call_count());
    println!("    pagerduty:    {}", pagerduty_provider.call_count());
    println!(
        "    slack:        {} (not reached)",
        slack_provider.call_count()
    );
    println!(
        "    logger:       {} (not reached)",
        logger_provider.call_count()
    );
    check_provider.assert_called(1);
    pagerduty_provider.assert_called(1);
    slack_provider.assert_not_called();
    logger_provider.assert_not_called();

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 2: Success/Failure Branching
    // =========================================================================
    //
    // Chain shape:
    //   validate ─┬─ success==true  ──> process ──(end)
    //             └─ success!=true  ──> reject  ──(end)
    //
    // "reject" is the last step so it naturally terminates. "process" uses
    // default_next to skip over "reject" to chain end. We demonstrate this
    // twice: once where validation succeeds and once where it fails.
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 2: SUCCESS/FAILURE BRANCHING");
    println!("------------------------------------------------------------------\n");

    // --- Run A: validation succeeds ---
    println!("  --- Run A: validation succeeds ---\n");

    let validate_ok = Arc::new(RecordingProvider::new("validator").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"valid": true}),
        ))
    }));
    let process_provider = Arc::new(RecordingProvider::new("processor"));
    let reject_provider = Arc::new(RecordingProvider::new("rejector"));
    let done2a_provider = Arc::new(RecordingProvider::new("done-svc"));

    let chain_config = ChainConfig::new("validate-chain")
        .with_step(
            ChainStepConfig::new(
                "validate",
                "validator",
                "validate",
                serde_json::json!({"data": "{{origin.payload}}"}),
            )
            .with_branch(BranchCondition::new(
                "success",
                BranchOperator::Eq,
                Some(serde_json::Value::Bool(true)),
                "process",
            ))
            .with_branch(BranchCondition::new(
                "success",
                BranchOperator::Neq,
                Some(serde_json::Value::Bool(true)),
                "reject",
            )),
        )
        .with_step(
            ChainStepConfig::new(
                "process",
                "processor",
                "process_data",
                serde_json::json!({}),
            )
            .with_default_next("done"),
        )
        .with_step(
            ChainStepConfig::new(
                "reject",
                "rejector",
                "send_rejection",
                serde_json::json!({}),
            )
            .with_default_next("done"),
        )
        .with_step(ChainStepConfig::new(
            "done",
            "done-svc",
            "ack",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rule_yaml_validate = r#"
rules:
  - name: validate-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "validate"
    action:
      type: chain
      chain: validate-chain
"#;

    let gateway = build_gateway(
        vec![
            Arc::clone(&validate_ok) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&process_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&reject_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&done2a_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config.clone(),
        rule_yaml_validate,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "validator",
        "validate",
        serde_json::json!({"record": "abc"}),
    )
    .await?;

    // Advance validate (succeeds) -> branches to "process"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 0 (validate) advanced -> process");

    // Advance process -> default_next "done"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 1 (process) advanced -> done");

    // Advance done -> chain completes
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 2 (done) advanced -> complete");

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["validate", "process", "done"]
    );
    process_provider.assert_called(1);
    reject_provider.assert_not_called();

    gateway.shutdown().await;
    println!("  PASSED\n");

    // --- Run B: validation fails ---
    println!("  --- Run B: validation fails ---\n");

    let validate_fail =
        Arc::new(RecordingProvider::new("validator").with_failure_mode(FailureMode::Always));
    let process_provider_b = Arc::new(RecordingProvider::new("processor"));
    let reject_provider_b = Arc::new(RecordingProvider::new("rejector"));
    let done2b_provider = Arc::new(RecordingProvider::new("done-svc"));

    let gateway = build_gateway(
        vec![
            Arc::clone(&validate_fail) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&process_provider_b) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&reject_provider_b) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&done2b_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        // Reuse the same chain config but with skip-on-failure for validate.
        ChainConfig::new("validate-chain")
            .with_step(
                ChainStepConfig::new("validate", "validator", "validate", serde_json::json!({}))
                    .with_on_failure(acteon_core::chain::StepFailurePolicy::Skip)
                    .with_branch(BranchCondition::new(
                        "success",
                        BranchOperator::Eq,
                        Some(serde_json::Value::Bool(true)),
                        "process",
                    ))
                    .with_branch(BranchCondition::new(
                        "success",
                        BranchOperator::Neq,
                        Some(serde_json::Value::Bool(true)),
                        "reject",
                    )),
            )
            .with_step(
                ChainStepConfig::new(
                    "process",
                    "processor",
                    "process_data",
                    serde_json::json!({}),
                )
                .with_default_next("done"),
            )
            .with_step(
                ChainStepConfig::new(
                    "reject",
                    "rejector",
                    "send_rejection",
                    serde_json::json!({}),
                )
                .with_default_next("done"),
            )
            .with_step(ChainStepConfig::new(
                "done",
                "done-svc",
                "ack",
                serde_json::json!({}),
            ))
            .with_timeout(30),
        rule_yaml_validate,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "validator",
        "validate",
        serde_json::json!({"record": "xyz"}),
    )
    .await?;

    // Advance validate -> fails (skip policy) -> branches to "reject"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 0 (validate) failed -> skip -> reject");

    // Advance reject -> default_next "done"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 1 (reject) advanced -> done");

    // Advance done -> chain completes
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 2 (done) advanced -> complete");

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["validate", "reject", "done"]
    );
    process_provider_b.assert_not_called();
    reject_provider_b.assert_called(1);

    gateway.shutdown().await;
    println!("  PASSED\n");

    // =========================================================================
    // DEMO 3: Multi-Step Branch Paths Converging
    // =========================================================================
    //
    // Chain shape:
    //   classify ─┬─ type=="A" ──> handle-a ──┐
    //             └─ type=="B" ──> handle-b ──┤
    //                                         └──> finalize ──(end)
    //
    // Both handle-a and handle-b use default_next to converge on "finalize".
    // We run this twice with different classification results to verify
    // that execution_path differs.
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 3: MULTI-STEP BRANCH PATHS CONVERGING");
    println!("------------------------------------------------------------------\n");

    let classify_provider = Arc::new(RecordingProvider::new("classifier").with_response_fn(
        |_action| {
            Ok(acteon_core::ProviderResponse::success(serde_json::json!({
                "type": "A"
            })))
        },
    ));
    let handle_a_provider = Arc::new(RecordingProvider::new("handler-a"));
    let handle_b_provider = Arc::new(RecordingProvider::new("handler-b"));
    let finalize_provider = Arc::new(RecordingProvider::new("finalizer"));

    let chain_config = ChainConfig::new("classify-chain")
        .with_step(
            ChainStepConfig::new("classify", "classifier", "classify", serde_json::json!({}))
                .with_branch(BranchCondition::new(
                    "body.type",
                    BranchOperator::Eq,
                    Some(serde_json::json!("A")),
                    "handle-a",
                ))
                .with_branch(BranchCondition::new(
                    "body.type",
                    BranchOperator::Eq,
                    Some(serde_json::json!("B")),
                    "handle-b",
                )),
        )
        .with_step(
            ChainStepConfig::new("handle-a", "handler-a", "process_a", serde_json::json!({}))
                .with_default_next("finalize"),
        )
        .with_step(
            ChainStepConfig::new("handle-b", "handler-b", "process_b", serde_json::json!({}))
                .with_default_next("finalize"),
        )
        .with_step(ChainStepConfig::new(
            "finalize",
            "finalizer",
            "finalize",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rule_yaml_classify = r#"
rules:
  - name: classify-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "classify"
    action:
      type: chain
      chain: classify-chain
"#;

    // --- Run with type A ---
    println!("  --- Run with type A ---\n");

    let gateway = build_gateway(
        vec![
            Arc::clone(&classify_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&handle_a_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&handle_b_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&finalize_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config.clone(),
        rule_yaml_classify,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "classifier",
        "classify",
        serde_json::json!({}),
    )
    .await?;

    // classify -> handle-a -> finalize -> complete
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 0 (classify) -> handle-a");

    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 1 (handle-a) -> finalize");

    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 2 (finalize) -> complete");

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["classify", "handle-a", "finalize"]
    );
    handle_a_provider.assert_called(1);
    handle_b_provider.assert_not_called();
    finalize_provider.assert_called(1);

    gateway.shutdown().await;
    println!("  PASSED\n");

    // --- Run with type B ---
    println!("  --- Run with type B ---\n");

    let classify_b_provider =
        Arc::new(RecordingProvider::new("classifier").with_response_fn(|_| {
            Ok(acteon_core::ProviderResponse::success(serde_json::json!({
                "type": "B"
            })))
        }));
    let handle_a2 = Arc::new(RecordingProvider::new("handler-a"));
    let handle_b2 = Arc::new(RecordingProvider::new("handler-b"));
    let finalize2 = Arc::new(RecordingProvider::new("finalizer"));

    let gateway = build_gateway(
        vec![
            Arc::clone(&classify_b_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&handle_a2) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&handle_b2) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&finalize2) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config,
        rule_yaml_classify,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "classifier",
        "classify",
        serde_json::json!({}),
    )
    .await?;

    // classify -> handle-b -> finalize -> complete
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 0 (classify) -> handle-b");

    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 1 (handle-b) -> finalize");

    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 2 (finalize) -> complete");

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["classify", "handle-b", "finalize"]
    );
    handle_a2.assert_not_called();
    handle_b2.assert_called(1);
    finalize2.assert_called(1);

    gateway.shutdown().await;
    println!("  PASSED\n");

    // =========================================================================
    // DEMO 4: Default Fallthrough
    // =========================================================================
    //
    // Chain shape:
    //   check ─┬─ priority=="high"   ──> handle-high ──> done
    //          ├─ priority=="medium" ──> handle-high ──> done
    //          └─ default            ──> fallback    ──> done
    //
    // The response has priority "low", so no branch matches and the
    // default_next step "fallback" is used instead.
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 4: DEFAULT FALLTHROUGH");
    println!("------------------------------------------------------------------\n");

    let check_provider_d4 = Arc::new(RecordingProvider::new("checker").with_response_fn(|_| {
        Ok(acteon_core::ProviderResponse::success(serde_json::json!({
            "priority": "low"
        })))
    }));
    let high_handler = Arc::new(RecordingProvider::new("high-handler"));
    let fallback_handler = Arc::new(RecordingProvider::new("fallback-svc"));
    let done4_provider = Arc::new(RecordingProvider::new("done-svc"));

    let chain_config = ChainConfig::new("fallthrough-chain")
        .with_step(
            ChainStepConfig::new("check", "checker", "check", serde_json::json!({}))
                .with_branch(BranchCondition::new(
                    "body.priority",
                    BranchOperator::Eq,
                    Some(serde_json::json!("high")),
                    "handle-high",
                ))
                .with_branch(BranchCondition::new(
                    "body.priority",
                    BranchOperator::Eq,
                    Some(serde_json::json!("medium")),
                    "handle-high",
                ))
                // Neither "high" nor "medium" match "low" -> default_next
                .with_default_next("fallback"),
        )
        .with_step(
            ChainStepConfig::new(
                "handle-high",
                "high-handler",
                "handle",
                serde_json::json!({}),
            )
            .with_default_next("done"),
        )
        .with_step(
            ChainStepConfig::new(
                "fallback",
                "fallback-svc",
                "fallback_action",
                serde_json::json!({}),
            )
            .with_default_next("done"),
        )
        .with_step(ChainStepConfig::new(
            "done",
            "done-svc",
            "ack",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rule_yaml_fallthrough = r#"
rules:
  - name: fallthrough-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "fallthrough"
    action:
      type: chain
      chain: fallthrough-chain
"#;

    let gateway = build_gateway(
        vec![
            Arc::clone(&check_provider_d4) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&high_handler) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&fallback_handler) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&done4_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config,
        rule_yaml_fallthrough,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "checker",
        "fallthrough",
        serde_json::json!({}),
    )
    .await?;
    println!("  Chain started: {chain_id}");

    // check -> no branch matches -> default_next "fallback"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 0 (check) -> no branch match -> fallback");

    // fallback -> default_next "done"
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 1 (fallback) -> done");

    // done -> complete
    gateway.advance_chain("app", "tenant-1", &chain_id).await?;
    println!("  Step 2 (done) -> complete");

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("\n  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(
        chain_state.execution_path,
        vec!["check", "fallback", "done"]
    );
    high_handler.assert_not_called();
    fallback_handler.assert_called(1);

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    // =========================================================================
    // DEMO 5: Linear Chain Backward Compatibility
    // =========================================================================
    //
    // A standard linear chain (no branches) still works exactly as before
    // and execution_path is populated correctly with the sequential step
    // order.
    // =========================================================================
    println!("------------------------------------------------------------------");
    println!("  DEMO 5: LINEAR CHAIN BACKWARD COMPATIBILITY");
    println!("------------------------------------------------------------------\n");

    let step_a_provider = Arc::new(RecordingProvider::new("svc-a"));
    let step_b_provider = Arc::new(RecordingProvider::new("svc-b"));
    let step_c_provider = Arc::new(RecordingProvider::new("svc-c"));

    let chain_config = ChainConfig::new("linear-chain")
        .with_step(ChainStepConfig::new(
            "first",
            "svc-a",
            "do_a",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "second",
            "svc-b",
            "do_b",
            serde_json::json!({}),
        ))
        .with_step(ChainStepConfig::new(
            "third",
            "svc-c",
            "do_c",
            serde_json::json!({}),
        ))
        .with_timeout(30);

    let rule_yaml_linear = r#"
rules:
  - name: linear-chain
    priority: 1
    condition:
      field: action.action_type
      eq: "linear"
    action:
      type: chain
      chain: linear-chain
"#;

    let gateway = build_gateway(
        vec![
            Arc::clone(&step_a_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&step_b_provider) as Arc<dyn acteon_provider::DynProvider>,
            Arc::clone(&step_c_provider) as Arc<dyn acteon_provider::DynProvider>,
        ],
        chain_config,
        rule_yaml_linear,
    )
    .await?;

    let chain_id = start_chain(
        &gateway,
        "app",
        "tenant-1",
        "svc-a",
        "linear",
        serde_json::json!({}),
    )
    .await?;
    println!("  Chain started: {chain_id}");

    for i in 0..3 {
        gateway.advance_chain("app", "tenant-1", &chain_id).await?;
        println!("  Step {i} advanced");
    }

    let chain_state = gateway
        .get_chain_status("app", "tenant-1", &chain_id)
        .await?
        .expect("chain state should exist");

    println!("\n  Final status: {:?}", chain_state.status);
    println!("  Execution path: {:?}", chain_state.execution_path);
    assert_eq!(
        chain_state.status,
        acteon_core::chain::ChainStatus::Completed
    );
    assert_eq!(chain_state.execution_path, vec!["first", "second", "third"]);
    step_a_provider.assert_called(1);
    step_b_provider.assert_called(1);
    step_c_provider.assert_called(1);

    // Verify step_results are populated for all steps.
    for (i, name) in ["first", "second", "third"].iter().enumerate() {
        let result = chain_state.step_results[i]
            .as_ref()
            .unwrap_or_else(|| panic!("step_results[{i}] should be Some"));
        assert!(result.success, "step '{name}' should have succeeded");
        assert_eq!(result.step_name, *name);
    }

    gateway.shutdown().await;
    println!("\n  PASSED\n");

    println!("==================================================================");
    println!("     ALL BRANCH CHAIN SIMULATIONS PASSED");
    println!("==================================================================");

    Ok(())
}
