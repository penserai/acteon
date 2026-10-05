use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{Action, ActionError, ActionOutcome, Attachment, ProviderResponse};
use acteon_executor::{
    ProviderExecutionAdmission, ProviderExecutionAuthority, ProviderExecutionMediator,
    ProviderInvocation, ProviderInvocationOrigin,
};
use acteon_gateway::{CircuitBreakerConfig, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;
use serde_json::json;

struct CountedProvider {
    name: &'static str,
    calls: AtomicUsize,
}
#[async_trait]
impl DynProvider for CountedProvider {
    fn name(&self) -> &str {
        self.name
    }
    async fn execute(&self, _: &Action) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderResponse::success(json!({"sent": true})))
    }
    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
    fn supports_attachments(&self) -> bool {
        true
    }
}
struct Captured {
    origin: ProviderInvocationOrigin,
    selected: Arc<dyn DynProvider>,
    action: Action,
    attachment_bytes: Vec<Vec<u8>>,
}
#[derive(Default)]
struct RefusingMediator {
    calls: Mutex<Vec<Captured>>,
}
#[async_trait]
impl ProviderExecutionMediator for RefusingMediator {
    async fn execute(&self, invocation: ProviderInvocation<'_>) -> ActionOutcome {
        self.calls.lock().unwrap().push(Captured {
            origin: invocation.origin,
            selected: invocation.selected.clone(),
            action: invocation.action.clone(),
            attachment_bytes: invocation.context.map_or_else(Vec::new, |ctx| {
                ctx.attachments.iter().map(|a| a.data.clone()).collect()
            }),
        });
        ActionOutcome::Failed(ActionError {
            code: "MEDIATION_REFUSED".into(),
            message: "original authority required".into(),
            retryable: false,
            attempts: 0,
        })
    }
}
fn action() -> Action {
    Action::new("city", "team", "primary", "operate", json!({"input": 1}))
}
fn providers() -> (Arc<CountedProvider>, Arc<CountedProvider>) {
    (
        Arc::new(CountedProvider {
            name: "primary",
            calls: AtomicUsize::new(0),
        }),
        Arc::new(CountedProvider {
            name: "secondary",
            calls: AtomicUsize::new(0),
        }),
    )
}
fn builder(
    primary: &Arc<CountedProvider>,
    secondary: &Arc<CountedProvider>,
    mediator: &Arc<RefusingMediator>,
) -> GatewayBuilder {
    GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(primary.clone())
        .provider(secondary.clone())
        .provider_execution_mediator(mediator.clone())
        .external_url("https://city.example.com")
}
fn assert_not_sent(primary: &CountedProvider, secondary: &CountedProvider) {
    assert_eq!(primary.calls.load(Ordering::SeqCst), 0);
    assert_eq!(secondary.calls.load(Ordering::SeqCst), 0);
}

struct NeverAdmit;
#[async_trait]
impl ProviderExecutionAdmission for NeverAdmit {
    async fn admit(
        &self,
        _: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        panic!("admission ran without an enforcing mediator")
    }
}
#[tokio::test]
async fn private_admission_cannot_silently_use_the_legacy_executor() {
    let (primary, secondary) = providers();
    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(primary.clone())
        .build()
        .unwrap();
    let ActionOutcome::Failed(error) = gateway
        .dispatch_with_execution_admission(action(), None, &NeverAdmit)
        .await
        .unwrap()
    else {
        panic!("legacy execution accepted private admission")
    };
    assert_eq!(error.code, "EXECUTION_MEDIATOR_REQUIRED");
    assert_not_sent(&primary, &secondary);
}

#[tokio::test]
async fn direct_modified_deduplicated_and_throttled_effects_cannot_bypass_mediation() {
    for rule in [
        None,
        Some(RuleAction::Modify {
            changes: json!({"input": 2}),
        }),
        Some(RuleAction::Deduplicate {
            ttl_seconds: Some(300),
        }),
        Some(RuleAction::Throttle {
            max_count: 5,
            window_seconds: 60,
        }),
    ] {
        let (primary, secondary) = providers();
        let mediator = Arc::new(RefusingMediator::default());
        let mut builder = builder(&primary, &secondary, &mediator);
        let modified = matches!(&rule, Some(RuleAction::Modify { .. }));
        if let Some(rule) = rule {
            builder = builder.rules(vec![Rule::new("policy", Expr::Bool(true), rule)]);
        }
        let gateway = builder.build().unwrap();
        let mut request = action();
        request.dedup_key = Some("one-operation".into());
        assert!(matches!(
            gateway.dispatch(request, None).await.unwrap(),
            ActionOutcome::Failed(_)
        ));
        let calls = mediator.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].origin, ProviderInvocationOrigin::Dispatch);
        let expected: Arc<dyn DynProvider> = primary.clone();
        assert!(Arc::ptr_eq(&calls[0].selected, &expected));
        assert_eq!(
            calls[0].action.payload["input"],
            if modified { 2 } else { 1 }
        );
        assert_not_sent(&primary, &secondary);
    }
}

#[tokio::test]
async fn rerouting_passes_the_actual_selected_instance_and_keeps_original_input_labels() {
    let (primary, secondary) = providers();
    let mediator = Arc::new(RefusingMediator::default());
    let gateway = builder(&primary, &secondary, &mediator)
        .rules(vec![Rule::new(
            "route",
            Expr::Bool(true),
            RuleAction::Reroute {
                target_provider: "secondary".into(),
            },
        )])
        .build()
        .unwrap();
    assert!(matches!(
        gateway.dispatch(action(), None).await.unwrap(),
        ActionOutcome::Failed(_)
    ));
    let calls = mediator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].origin, ProviderInvocationOrigin::Reroute);
    let expected: Arc<dyn DynProvider> = secondary.clone();
    assert!(Arc::ptr_eq(&calls[0].selected, &expected));
    assert_eq!(calls[0].action.provider.as_str(), "primary");
    assert_not_sent(&primary, &secondary);
}

#[tokio::test]
async fn circuit_fallback_is_mediated_and_refusal_never_calls_the_provider() {
    let (primary, secondary) = providers();
    let mediator = Arc::new(RefusingMediator::default());
    let gateway = builder(&primary, &secondary, &mediator)
        .circuit_breaker_provider(
            "primary",
            CircuitBreakerConfig {
                failure_threshold: 1,
                success_threshold: 1,
                recovery_timeout: Duration::from_secs(3600),
                fallback_provider: Some("secondary".into()),
            },
        )
        .build()
        .unwrap();
    assert!(matches!(
        gateway.dispatch(action(), None).await.unwrap(),
        ActionOutcome::Failed(_)
    ));
    let primary_circuit = gateway.circuit_breakers().unwrap().get("primary").unwrap();
    assert_eq!(
        primary_circuit.state().await,
        acteon_gateway::CircuitState::Closed
    );
    // A real provider-health event, separate from authorization refusal.
    primary_circuit.record_failure().await;

    assert!(matches!(
        gateway.dispatch(action(), None).await.unwrap(),
        ActionOutcome::Failed(_)
    ));
    let calls = mediator.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].origin, ProviderInvocationOrigin::Fallback);
    let expected: Arc<dyn DynProvider> = secondary.clone();
    assert!(Arc::ptr_eq(&calls[1].selected, &expected));
    assert_not_sent(&primary, &secondary);
}

#[tokio::test]
async fn approval_notifications_and_background_retries_use_the_same_boundary() {
    let (primary, secondary) = providers();
    let mediator = Arc::new(RefusingMediator::default());
    let gateway = builder(&primary, &secondary, &mediator)
        .rules(vec![Rule::new(
            "approve",
            Expr::Bool(true),
            RuleAction::RequestApproval {
                notify_provider: "secondary".into(),
                timeout_seconds: 3600,
                message: None,
            },
        )])
        .build()
        .unwrap();
    let ActionOutcome::PendingApproval {
        approval_id,
        notification_sent,
        ..
    } = gateway.dispatch(action(), None).await.unwrap()
    else {
        panic!("approval required");
    };
    assert!(!notification_sent);
    assert!(
        !gateway
            .retry_approval_notification("city", "team", &approval_id)
            .await
            .unwrap()
    );
    let calls = mediator.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].origin,
        ProviderInvocationOrigin::ApprovalNotification
    );
    assert_eq!(calls[1].origin, ProviderInvocationOrigin::ApprovalRetry);
    assert!(
        calls
            .iter()
            .all(|call| call.action.action_type == "approval_notification")
    );
    assert_not_sent(&primary, &secondary);
}

#[tokio::test]
async fn resolved_attachments_reach_mediation_and_dry_run_does_not_invoke_it() {
    let (primary, secondary) = providers();
    let mediator = Arc::new(RefusingMediator::default());
    let gateway = builder(&primary, &secondary, &mediator).build().unwrap();
    let mut request = action();
    request.attachments.push(Attachment {
        id: "one".into(),
        name: "note".into(),
        filename: "note.txt".into(),
        content_type: "text/plain".into(),
        data_base64: "aGVsbG8=".into(),
    });
    assert!(matches!(
        gateway
            .dispatch_dry_run(request.clone(), None)
            .await
            .unwrap(),
        ActionOutcome::DryRun { .. }
    ));
    assert!(mediator.calls.lock().unwrap().is_empty());
    assert!(matches!(
        gateway.dispatch(request, None).await.unwrap(),
        ActionOutcome::Failed(_)
    ));
    let calls = mediator.calls.lock().unwrap();
    assert_eq!(calls[0].attachment_bytes, vec![b"hello".to_vec()]);
    assert_not_sent(&primary, &secondary);
}
