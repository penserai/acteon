//! Integration tests for gateway-level recurring overlap enforcement:
//! `Gateway::enforce_overlap_policy` must make the same decisions for any
//! consumer (the bundled server's recurring consumer or an embedder).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use acteon_core::chain::{ChainConfig, ChainStepConfig, SignalStepConfig};
use acteon_core::{
    Action, ActionOutcome, ChainStatus, OverlapPolicy, ProviderResponse, RecurringAction,
    RecurringActionTemplate,
};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder, OverlapDecision};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const NS: &str = "notifications";
const TENANT: &str = "tenant-1";

struct MockProvider;

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &'static str {
        "email"
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn build_gateway(chain: ChainConfig) -> Gateway {
    let rule = Rule::new(
        "start-chain",
        Expr::Binary(
            BinaryOp::Eq,
            Box::new(Expr::Field(
                Box::new(Expr::Ident("action".into())),
                "action_type".into(),
            )),
            Box::new(Expr::String("start_chain".into())),
        ),
        RuleAction::Chain {
            chain: "test-chain".into(),
        },
    );
    GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(vec![rule])
        .provider(Arc::new(MockProvider))
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .chain(chain)
        .build()
        .expect("gateway should build")
}

/// A chain that parks indefinitely on a signal (stays active).
fn parked_chain() -> ChainConfig {
    ChainConfig::new("test-chain").with_step(ChainStepConfig::new_wait_for_signal(
        "wait",
        SignalStepConfig {
            signal_name: "go".into(),
            timeout_seconds: None,
            on_timeout: None,
        },
    ))
}

/// A chain with one provider step (completes on first advance).
fn quick_chain() -> ChainConfig {
    ChainConfig::new("test-chain").with_step(ChainStepConfig::new(
        "notify",
        "email",
        "send_email",
        serde_json::json!({}),
    ))
}

async fn start_chain(gateway: &Gateway) -> String {
    let action = Action::new(NS, TENANT, "email", "start_chain", serde_json::json!({}));
    match gateway.dispatch(action, None).await.unwrap() {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id,
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

fn recurring(policy: OverlapPolicy, last_execution_id: Option<String>) -> RecurringAction {
    let now = Utc::now();
    RecurringAction {
        id: "rec-1".into(),
        namespace: NS.into(),
        tenant: TENANT.into(),
        cron_expr: "*/5 * * * *".into(),
        timezone: "UTC".into(),
        enabled: true,
        action_template: RecurringActionTemplate {
            provider: "email".into(),
            action_type: "start_chain".into(),
            payload: serde_json::json!({}),
            metadata: HashMap::new(),
            dedup_key: None,
        },
        created_at: now,
        updated_at: now,
        last_executed_at: None,
        next_execution_at: None,
        ends_at: None,
        max_executions: None,
        execution_count: 0,
        description: None,
        labels: HashMap::new(),
        overlap_policy: policy,
        last_execution_id,
    }
}

#[tokio::test]
async fn allow_all_always_proceeds() {
    let gateway = build_gateway(parked_chain());
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();

    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::AllowAll, Some(chain_id)),
        )
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);
}

#[tokio::test]
async fn skip_drops_occurrence_while_previous_is_active() {
    let gateway = build_gateway(parked_chain());
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingSignal);

    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::Skip, Some(chain_id.clone())),
        )
        .await;
    assert_eq!(
        decision,
        OverlapDecision::Skip {
            previous_execution_id: chain_id
        }
    );
    assert_eq!(gateway.metrics().snapshot().recurring_skipped, 1);
}

#[tokio::test]
async fn skip_proceeds_once_previous_settles() {
    let gateway = build_gateway(quick_chain());
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);

    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::Skip, Some(chain_id)),
        )
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);
    assert_eq!(gateway.metrics().snapshot().recurring_skipped, 0);
}

#[tokio::test]
async fn skip_proceeds_without_a_previous_execution() {
    let gateway = build_gateway(parked_chain());

    // No previous execution recorded.
    let decision = gateway
        .enforce_overlap_policy(NS, TENANT, "rec-1", &recurring(OverlapPolicy::Skip, None))
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);

    // A recorded ID whose execution no longer exists (e.g. expired TTL).
    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::Skip, Some("gone".into())),
        )
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);
}

#[tokio::test]
async fn cancel_other_cancels_previous_then_proceeds() {
    let gateway = build_gateway(parked_chain());
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();

    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::CancelOther, Some(chain_id.clone())),
        )
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);

    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Cancelled);
    assert_eq!(state.cancelled_by.as_deref(), Some("recurring:rec-1"));
    assert_eq!(
        state.cancel_reason.as_deref(),
        Some("superseded by next recurring occurrence")
    );
}

#[tokio::test]
async fn cancel_other_proceeds_when_previous_already_settled() {
    let gateway = build_gateway(quick_chain());
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();

    let decision = gateway
        .enforce_overlap_policy(
            NS,
            TENANT,
            "rec-1",
            &recurring(OverlapPolicy::CancelOther, Some(chain_id.clone())),
        )
        .await;
    assert_eq!(decision, OverlapDecision::Proceed);

    // Completed executions are left untouched.
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
}
