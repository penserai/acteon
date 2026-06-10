//! Integration tests for durable-execution features: timer steps,
//! wait-for-signal steps, execution history, definition pinning /
//! versioning, search attributes, and visibility queries.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use acteon_core::chain::{
    ChainConfig, ChainStepConfig, SignalStepConfig, TimerStepConfig, WaitState,
};
use acteon_core::{Action, ActionOutcome, ChainStatus, ExecutionEventType, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{ExecutionFilter, Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

struct MockProvider {
    provider_name: String,
}

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn chain_rule(chain: &str) -> Rule {
    Rule::new(
        format!("start-{chain}"),
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("start_chain".into())),
        ),
        RuleAction::Chain {
            chain: chain.into(),
        },
    )
}

fn build_gateway(chains: Vec<ChainConfig>) -> Gateway {
    let store = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());
    let mut builder = GatewayBuilder::new()
        .state(store)
        .lock(lock)
        .rules(vec![chain_rule("test-chain")])
        .provider(Arc::new(MockProvider {
            provider_name: "email".into(),
        }))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        });
    for chain in chains {
        builder = builder.chain(chain);
    }
    builder.build().expect("gateway should build")
}

fn start_action() -> Action {
    let mut action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "start_chain",
        serde_json::json!({"request": "do-it"}),
    );
    action
        .metadata
        .labels
        .insert("team".into(), "payments".into());
    action
}

async fn start_chain(gateway: &Gateway) -> String {
    match gateway.dispatch(start_action(), None).await.unwrap() {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id,
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

fn email_step(name: &str) -> ChainStepConfig {
    ChainStepConfig::new(name, "email", "send_email", serde_json::json!({"s": name}))
}

// -- Timer steps -------------------------------------------------------------

#[tokio::test]
async fn timer_step_pauses_then_fires() {
    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_timer(
            "wait",
            TimerStepConfig {
                duration_seconds: Some(1),
                until: None,
            },
        ))
        .with_step(email_step("notify"));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;

    // First advancement arms the timer and pauses the chain.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingTimer);
    assert!(matches!(
        state.wait_state,
        Some(WaitState::Timer { step_index: 0, .. })
    ));

    // A premature wake leaves the chain waiting.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingTimer);

    // After the fire time, the timer resolves and the chain advances.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    // Run the provider step.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert!(state.wait_state.is_none());
    assert!(state.step_results[0].as_ref().unwrap().success);

    let history = gateway
        .get_execution_history("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let kinds: Vec<&str> = history
        .events
        .iter()
        .map(|e| match &e.event {
            ExecutionEventType::ExecutionStarted { .. } => "started",
            ExecutionEventType::TimerStarted { .. } => "timer_started",
            ExecutionEventType::TimerFired { .. } => "timer_fired",
            ExecutionEventType::StepCompleted { .. } => "step_completed",
            ExecutionEventType::ExecutionCompleted => "completed",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "started",
            "timer_started",
            "timer_fired",
            "step_completed",
            "step_completed",
            "completed"
        ],
        "unexpected history: {kinds:?}"
    );
}

#[tokio::test]
async fn timer_step_with_past_until_fires_on_next_advance() {
    let chain = ChainConfig::new("test-chain").with_step(ChainStepConfig::new_timer(
        "wait",
        TimerStepConfig {
            duration_seconds: None,
            until: Some(Utc::now() - chrono::Duration::seconds(5)),
        },
    ));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
}

// -- Signal steps ------------------------------------------------------------

fn signal_chain_config(timeout_seconds: Option<u64>, on_timeout: Option<&str>) -> ChainConfig {
    let mut config = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_wait_for_signal(
            "wait-approval",
            SignalStepConfig {
                signal_name: "approved".into(),
                timeout_seconds,
                on_timeout: on_timeout.map(Into::into),
            },
        ))
        .with_step(email_step("notify"));
    if on_timeout.is_some() {
        config = config.with_step(email_step("escalate"));
    }
    config
}

#[tokio::test]
async fn signal_wakes_waiting_chain_and_carries_payload() {
    let gateway = build_gateway(vec![signal_chain_config(None, None)]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingSignal);

    gateway
        .signal_chain(
            "notifications",
            "tenant-1",
            &chain_id,
            "approved",
            serde_json::json!({"approver": "renzo"}),
        )
        .await
        .unwrap();

    // The signal indexes the chain ready; drive it manually as the
    // background processor would.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    let wait_result = state.step_results[0].as_ref().unwrap();
    assert!(wait_result.success);
    assert_eq!(
        wait_result.response_body,
        Some(serde_json::json!({"approver": "renzo"}))
    );
}

#[tokio::test]
async fn signal_delivered_before_wait_step_is_buffered() {
    let gateway = build_gateway(vec![signal_chain_config(None, None)]);
    let chain_id = start_chain(&gateway).await;

    // Signal arrives before the chain reaches the wait step.
    gateway
        .signal_chain(
            "notifications",
            "tenant-1",
            &chain_id,
            "approved",
            serde_json::json!({"early": true}),
        )
        .await
        .unwrap();

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert_eq!(
        state.step_results[0].as_ref().unwrap().response_body,
        Some(serde_json::json!({"early": true}))
    );
}

#[tokio::test]
async fn signal_timeout_jumps_to_on_timeout_step() {
    let gateway = build_gateway(vec![signal_chain_config(Some(1), Some("escalate"))]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // Timeout fires: jump to the escalate step.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    // Execute the escalate step (terminal — it's the last step).
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert_eq!(
        state.execution_path,
        vec!["wait-approval", "escalate"],
        "expected the timeout branch to be taken"
    );
    let wait_result = state.step_results[0].as_ref().unwrap();
    assert!(!wait_result.success);
}

#[tokio::test]
async fn signal_timeout_without_target_fails_chain() {
    let gateway = build_gateway(vec![signal_chain_config(Some(1), None)]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Failed);
}

#[tokio::test]
async fn signal_to_unknown_chain_errors() {
    let gateway = build_gateway(vec![signal_chain_config(None, None)]);
    let err = gateway
        .signal_chain(
            "notifications",
            "tenant-1",
            "no-such-chain",
            "approved",
            serde_json::Value::Null,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// -- Versioning / definition pinning ------------------------------------------

#[tokio::test]
async fn inflight_execution_pins_definition_version() {
    let chain = ChainConfig::new("test-chain")
        .with_step(email_step("one"))
        .with_step(email_step("two"));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;

    // Execute the first step.
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    // Replace the definition with a single differently-named step. The
    // version bumps and in-flight executions must be unaffected.
    let replacement = ChainConfig::new("test-chain").with_step(email_step("replacement"));
    gateway.set_chain_config(replacement).unwrap();
    assert_eq!(gateway.chain_config("test-chain").unwrap().version, 2);

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert_eq!(state.chain_version, 1);
    assert_eq!(state.total_steps, 2);
    assert_eq!(state.execution_path, vec!["one", "two"]);

    // A new execution uses the replaced (v2) definition.
    let new_chain_id = start_chain(&gateway).await;
    gateway
        .advance_chain("notifications", "tenant-1", &new_chain_id)
        .await
        .unwrap();
    let new_state = gateway
        .get_chain_status("notifications", "tenant-1", &new_chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_state.chain_version, 2);
    assert_eq!(new_state.execution_path, vec!["replacement"]);
    assert_eq!(new_state.status, ChainStatus::Completed);
}

// -- Visibility ----------------------------------------------------------------

#[tokio::test]
async fn list_executions_filters_by_status_and_attributes() {
    let chain = ChainConfig::new("test-chain").with_step(ChainStepConfig::new_wait_for_signal(
        "wait",
        SignalStepConfig {
            signal_name: "go".into(),
            timeout_seconds: None,
            on_timeout: None,
        },
    ));
    let gateway = build_gateway(vec![chain]);

    let waiting_id = start_chain(&gateway).await;
    gateway
        .advance_chain("notifications", "tenant-1", &waiting_id)
        .await
        .unwrap();

    let completed_id = start_chain(&gateway).await;
    gateway
        .signal_chain(
            "notifications",
            "tenant-1",
            &completed_id,
            "go",
            serde_json::Value::Null,
        )
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &completed_id)
        .await
        .unwrap();

    // Search attributes are seeded from the origin action's metadata.
    let all = gateway
        .list_executions("notifications", "tenant-1", &ExecutionFilter::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter().all(|e| e.search_attributes.get("team")
            == Some(&serde_json::Value::String("payments".into())))
    );

    let waiting = gateway
        .list_executions(
            "notifications",
            "tenant-1",
            &ExecutionFilter {
                status: Some(ChainStatus::WaitingSignal),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].chain_id, waiting_id);

    // Upsert a custom attribute and filter on it.
    gateway
        .upsert_search_attributes(
            "notifications",
            "tenant-1",
            &completed_id,
            HashMap::from([("priority".to_owned(), serde_json::json!("high"))]),
        )
        .await
        .unwrap();
    let high = gateway
        .list_executions(
            "notifications",
            "tenant-1",
            &ExecutionFilter {
                attributes: vec![("priority".into(), "high".into())],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(high.len(), 1);
    assert_eq!(high[0].chain_id, completed_id);
}

// -- History on failure paths ---------------------------------------------------

#[tokio::test]
async fn history_records_terminal_failure_on_cancel() {
    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_timer(
            "wait",
            TimerStepConfig {
                duration_seconds: Some(3600),
                until: None,
            },
        ))
        .with_step(email_step("notify"));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .cancel_chain(
            "notifications",
            "tenant-1",
            &chain_id,
            Some("operator request".into()),
            Some("renzo".into()),
        )
        .await
        .unwrap();

    let history = gateway
        .get_execution_history("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    assert!(history.events.iter().any(|e| matches!(
        &e.event,
        ExecutionEventType::ExecutionCancelled { reason: Some(r) } if r == "operator request"
    )));
}

// -- Validation ------------------------------------------------------------------

#[test]
fn validation_rejects_conflicting_step_kinds() {
    let mut step = ChainStepConfig::new("bad", "email", "send", serde_json::json!({}));
    step.timer = Some(TimerStepConfig {
        duration_seconds: Some(5),
        until: None,
    });
    let config = ChainConfig::new("c").with_step(step);
    let errors = config.validate();
    assert!(
        errors.iter().any(|e| e.contains("mutually exclusive")),
        "expected mutual-exclusivity error, got {errors:?}"
    );
}

#[test]
fn validation_rejects_timer_without_duration_or_until() {
    let config = ChainConfig::new("c").with_step(ChainStepConfig::new_timer(
        "t",
        TimerStepConfig {
            duration_seconds: None,
            until: None,
        },
    ));
    let errors = config.validate();
    assert!(errors.iter().any(|e| e.contains("exactly one")));
}

#[test]
fn validation_rejects_signal_on_timeout_to_unknown_step() {
    let config = ChainConfig::new("c").with_step(ChainStepConfig::new_wait_for_signal(
        "w",
        SignalStepConfig {
            signal_name: "s".into(),
            timeout_seconds: Some(5),
            on_timeout: Some("ghost".into()),
        },
    ));
    let errors = config.validate();
    assert!(errors.iter().any(|e| e.contains("ghost")));
}

// -- Execution reset (replay from step) -----------------------------------------

#[tokio::test]
async fn reset_reruns_terminal_execution_from_step() {
    let chain = ChainConfig::new("test-chain")
        .with_step(email_step("one"))
        .with_step(email_step("two"));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);

    // Reset to the second step: the first step's result must be preserved,
    // the second cleared and re-run.
    let reset = gateway
        .reset_execution(
            "notifications",
            "tenant-1",
            &chain_id,
            "two",
            Some("operator re-run".into()),
        )
        .await
        .unwrap();
    assert_eq!(reset.status, ChainStatus::Running);
    assert_eq!(reset.current_step, 1);
    assert!(reset.step_results[0].is_some(), "step one result preserved");
    assert!(reset.step_results[1].is_none(), "step two cleared");
    assert_eq!(reset.execution_path, vec!["one", "two"]);

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert!(state.step_results[1].is_some());

    // The reset is on the record.
    let history = gateway
        .get_execution_history("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    assert!(history.events.iter().any(|e| matches!(
        &e.event,
        ExecutionEventType::ExecutionReset { step_name, reason: Some(r), .. }
            if step_name == "two" && r == "operator re-run"
    )));
}

#[tokio::test]
async fn reset_abandons_signal_wait() {
    let gateway = build_gateway(vec![signal_chain_config(None, None)]);
    let chain_id = start_chain(&gateway).await;

    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingSignal);

    // Reset to the same wait step: the wait is re-armed fresh.
    let reset = gateway
        .reset_execution(
            "notifications",
            "tenant-1",
            &chain_id,
            "wait-approval",
            None,
        )
        .await
        .unwrap();
    assert_eq!(reset.status, ChainStatus::Running);
    assert!(reset.wait_state.is_none());

    // The execution still works end-to-end after the reset.
    gateway
        .signal_chain(
            "notifications",
            "tenant-1",
            &chain_id,
            "approved",
            serde_json::json!({"ok": true}),
        )
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();
    let state = gateway
        .get_chain_status("notifications", "tenant-1", &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
}

#[tokio::test]
async fn reset_rejects_unreached_and_unknown_steps() {
    let chain = ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_wait_for_signal(
            "wait",
            SignalStepConfig {
                signal_name: "go".into(),
                timeout_seconds: None,
                on_timeout: None,
            },
        ))
        .with_step(email_step("after"));
    let gateway = build_gateway(vec![chain]);
    let chain_id = start_chain(&gateway).await;
    gateway
        .advance_chain("notifications", "tenant-1", &chain_id)
        .await
        .unwrap();

    // "after" was never reached (the chain is parked on the wait step).
    let err = gateway
        .reset_execution("notifications", "tenant-1", &chain_id, "after", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("never reached"));

    let err = gateway
        .reset_execution("notifications", "tenant-1", &chain_id, "ghost", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found in chain"));
}
