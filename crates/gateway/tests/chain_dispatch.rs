use std::sync::{Arc, Mutex};
use std::time::Duration;

use acteon_core::chain::{ChainConfig, ChainStepConfig, DispatchStepConfig};
use acteon_core::{Action, ActionOutcome, ChainStatus, ProviderResponse};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;

const NAMESPACE: &str = "observability";
const TENANT: &str = "acme";

#[derive(Default)]
struct RecordingProvider {
    actions: Mutex<Vec<Action>>,
}

#[async_trait]
impl DynProvider for RecordingProvider {
    fn name(&self) -> &str {
        "sink"
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(ProviderResponse::success(
            serde_json::json!({"seen": action.payload}),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn action_type_is(value: &str) -> Expr {
    Expr::Binary(
        BinaryOp::Eq,
        Box::new(Expr::Field(
            Box::new(Expr::Ident("action".into())),
            "action_type".into(),
        )),
        Box::new(Expr::String(value.into())),
    )
}

fn gateway(
    rules: Vec<Rule>,
    chains: Vec<ChainConfig>,
    provider: Arc<RecordingProvider>,
) -> Gateway {
    let mut builder = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(rules)
        .provider(provider)
        .executor_config(ExecutorConfig {
            max_retries: 0,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        });
    for chain in chains {
        builder = builder.chain(chain);
    }
    builder.build().unwrap()
}

async fn start(gateway: &Gateway, action_type: &str) -> (String, String) {
    let mut action = Action::new(
        NAMESPACE,
        TENANT,
        "sink",
        action_type,
        serde_json::json!({"score": 0.93}),
    );
    action
        .metadata
        .labels
        .insert("team".into(), "checkout".into());
    action
        .metadata
        .labels
        .insert("acteon.chain.ancestry".into(), r#"["forged"]"#.into());
    action
        .metadata
        .labels
        .insert("acteon.chain.parent_id".into(), "forged-parent".into());
    action
        .metadata
        .labels
        .insert("acteon.chain.parent_step".into(), "99".into());
    action
        .metadata
        .labels
        .insert("acteon.chain.root_action_id".into(), "forged-root".into());
    action.trace_context.insert(
        "traceparent".into(),
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
    );
    let root_action_id = action.id.to_string();
    let ActionOutcome::ChainStarted { chain_id, .. } =
        gateway.dispatch(action, None).await.unwrap()
    else {
        panic!("expected chain to start")
    };
    (chain_id, root_action_id)
}

#[tokio::test]
async fn dispatch_step_reenters_rules_and_preserves_causality() {
    let provider = Arc::new(RecordingProvider::default());
    let chain = ChainConfig::new("detector").with_step(ChainStepConfig::new_dispatch(
        "route",
        DispatchStepConfig {
            provider: "sink".into(),
            action_type: "detector.verdict".into(),
            dedup_key: Some("{{chain_id}}:policy-v2".into()),
            inherit_metadata: true,
        },
        serde_json::json!({"score": "{{origin.payload.score}}"}),
    ));
    let rules = vec![
        Rule::new(
            "start-detector",
            action_type_is("start_detector"),
            RuleAction::Chain {
                chain: "detector".into(),
            },
        )
        .with_priority(0),
        Rule::new(
            "mark-policy",
            action_type_is("detector.verdict"),
            RuleAction::Modify {
                changes: serde_json::json!({"policy_checked": true}),
            },
        )
        .with_priority(1),
    ];
    let gateway = gateway(rules, vec![chain], provider.clone());
    let (chain_id, root_action_id) = start(&gateway, "start_detector").await;

    gateway
        .advance_chain(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    let result = state.step_results[0].as_ref().unwrap();
    assert_eq!(
        result.response_body.as_ref().unwrap()["outcome"],
        "executed"
    );
    assert_eq!(
        result.response_body.as_ref().unwrap()["body"]["seen"]["policy_checked"],
        true
    );

    let actions = provider.actions.lock().unwrap();
    assert_eq!(actions.len(), 1);
    let emitted = &actions[0];
    assert!(emitted.id.as_str().starts_with("chain-dispatch-"));
    let expected_dedup_key = format!("{chain_id}:policy-v2");
    assert_eq!(
        emitted.dedup_key.as_deref(),
        Some(expected_dedup_key.as_str())
    );
    assert_eq!(emitted.payload["score"], 0.93);
    assert_eq!(emitted.payload["policy_checked"], true);
    assert_eq!(emitted.metadata.labels["team"], "checkout");
    assert_eq!(emitted.metadata.labels["acteon.chain.parent_id"], chain_id);
    assert_eq!(emitted.metadata.labels["acteon.chain.parent_step"], "0");
    assert_eq!(
        emitted.metadata.labels["acteon.chain.ancestry"],
        r#"["detector"]"#
    );
    assert_eq!(
        emitted.metadata.labels["acteon.chain.root_action_id"],
        root_action_id
    );
    assert_eq!(
        emitted.trace_context.get("traceparent"),
        Some(&"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned())
    );
}

#[tokio::test]
async fn dispatch_step_links_child_and_rejects_chain_cycle() {
    let provider = Arc::new(RecordingProvider::default());
    let parent = ChainConfig::new("parent").with_step(ChainStepConfig::new_dispatch(
        "spawn-child",
        DispatchStepConfig {
            provider: "sink".into(),
            action_type: "spawn_child".into(),
            dedup_key: None,
            inherit_metadata: false,
        },
        serde_json::json!({}),
    ));
    let child = ChainConfig::new("child").with_step(ChainStepConfig::new_dispatch(
        "reenter-child",
        DispatchStepConfig {
            provider: "sink".into(),
            action_type: "spawn_child".into(),
            dedup_key: None,
            inherit_metadata: false,
        },
        serde_json::json!({}),
    ));
    let rules = vec![
        Rule::new(
            "start-parent",
            action_type_is("start_parent"),
            RuleAction::Chain {
                chain: "parent".into(),
            },
        )
        .with_priority(0),
        Rule::new(
            "spawn-child",
            action_type_is("spawn_child"),
            RuleAction::Chain {
                chain: "child".into(),
            },
        )
        .with_priority(1),
    ];
    let gateway = gateway(rules, vec![parent, child], provider);
    let (parent_id, root_action_id) = start(&gateway, "start_parent").await;

    gateway
        .advance_chain(NAMESPACE, TENANT, &parent_id)
        .await
        .unwrap();
    let parent = gateway
        .get_chain_status(NAMESPACE, TENANT, &parent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, ChainStatus::Completed);
    assert!(parent.parent_chain_id.is_none());
    assert_eq!(parent.child_chain_ids.len(), 1);

    let child_id = &parent.child_chain_ids[0];
    let child = gateway
        .get_chain_status(NAMESPACE, TENANT, child_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.parent_chain_id.as_deref(), Some(parent_id.as_str()));
    assert_eq!(child.parent_step_index, Some(0));
    assert_eq!(
        child.origin_action.metadata.labels["acteon.chain.ancestry"],
        r#"["parent","child"]"#
    );
    assert_eq!(
        child.origin_action.metadata.labels["acteon.chain.root_action_id"],
        root_action_id
    );
    assert!(!child.origin_action.metadata.labels.contains_key("team"));

    gateway
        .advance_chain(NAMESPACE, TENANT, child_id)
        .await
        .unwrap();
    let child = gateway
        .get_chain_status(NAMESPACE, TENANT, child_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.status, ChainStatus::Failed);
    assert!(child.child_chain_ids.is_empty());
    assert_eq!(
        child.step_results[0].as_ref().unwrap().error.as_deref(),
        Some("chain dispatch rejected by ancestry or depth policy")
    );
}

#[tokio::test]
async fn dispatch_step_rejects_a_ninth_chain_in_the_ancestry() {
    let provider = Arc::new(RecordingProvider::default());
    let mut chains = Vec::new();
    let mut rules = vec![
        Rule::new(
            "start-depth-chain",
            action_type_is("start_depth_chain"),
            RuleAction::Chain { chain: "c0".into() },
        )
        .with_priority(0),
    ];

    for index in 0..9 {
        let next_action = format!("start_c{}", index + 1);
        chains.push(ChainConfig::new(format!("c{index}")).with_step(
            ChainStepConfig::new_dispatch(
                "next",
                DispatchStepConfig {
                    provider: "sink".into(),
                    action_type: next_action.clone(),
                    dedup_key: None,
                    inherit_metadata: false,
                },
                serde_json::json!({}),
            ),
        ));
        if index < 8 {
            rules.push(
                Rule::new(
                    format!("route-c{}", index + 1),
                    action_type_is(&next_action),
                    RuleAction::Chain {
                        chain: format!("c{}", index + 1),
                    },
                )
                .with_priority(index + 1),
            );
        }
    }

    let gateway = gateway(rules, chains, provider);
    let (mut chain_id, _) = start(&gateway, "start_depth_chain").await;

    for depth in 0..8 {
        gateway
            .advance_chain(NAMESPACE, TENANT, &chain_id)
            .await
            .unwrap();
        let state = gateway
            .get_chain_status(NAMESPACE, TENANT, &chain_id)
            .await
            .unwrap()
            .unwrap();
        if depth < 7 {
            assert_eq!(state.status, ChainStatus::Completed);
            assert_eq!(state.child_chain_ids.len(), 1);
            chain_id.clone_from(&state.child_chain_ids[0]);
        } else {
            assert_eq!(state.status, ChainStatus::Failed);
            assert!(state.child_chain_ids.is_empty());
            assert_eq!(
                state.step_results[0].as_ref().unwrap().error.as_deref(),
                Some("chain dispatch rejected by ancestry or depth policy")
            );
        }
    }
}

#[tokio::test]
async fn dispatch_step_rejects_an_unresolved_dedup_key() {
    let provider = Arc::new(RecordingProvider::default());
    let chain = ChainConfig::new("invalid-dedup").with_step(ChainStepConfig::new_dispatch(
        "route",
        DispatchStepConfig {
            provider: "sink".into(),
            action_type: "detector.verdict".into(),
            dedup_key: Some("{{origin.payload.missing}}".into()),
            inherit_metadata: true,
        },
        serde_json::json!({}),
    ));
    let rules = vec![Rule::new(
        "start-invalid-dedup",
        action_type_is("start_invalid_dedup"),
        RuleAction::Chain {
            chain: "invalid-dedup".into(),
        },
    )];
    let gateway = gateway(rules, vec![chain], provider.clone());
    let (chain_id, _) = start(&gateway, "start_invalid_dedup").await;

    gateway
        .advance_chain(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap();

    let state = gateway
        .get_chain_status(NAMESPACE, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Failed);
    assert_eq!(
        state.step_results[0].as_ref().unwrap().error.as_deref(),
        Some("dispatch dedup_key template must resolve to a non-empty string")
    );
    assert_eq!(provider.actions.lock().unwrap().len(), 0);
}
