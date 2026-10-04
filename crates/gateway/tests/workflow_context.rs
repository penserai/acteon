use std::{collections::HashMap, sync::Arc, time::Duration};

use acteon_core::{
    ParentClosePolicy, PrincipalIdentity, PrincipalKind, WorkerTaskStatus, WorkflowDirective,
    WorkflowStatus,
};
use acteon_gateway::{
    Gateway, GatewayBuilder,
    workflow_context::{workflow_context_digest, workflow_context_effect},
};
use acteon_governance::{
    AuthorityCoordinator, CoordinatorLimits,
    context::{
        ContextBinding, ContextSigningKey, ExecutionContextHandle, RootContextAdmission,
        TrustedContextStore, VerifiedExecutionContext,
    },
};
use acteon_state::testing::faults::{FaultStore, FaultTiming, WriteOperation};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::ManualClock;
use chrono::{DateTime, Utc};
use uuid::Uuid;

const NS: &str = "city";
const TENANT: &str = "tenant";

fn gateway(
    store: Arc<dyn StateStore>,
    clock: Arc<ManualClock>,
    contexts: Option<Arc<TrustedContextStore>>,
) -> Gateway {
    let builder = GatewayBuilder::new()
        .state(store)
        .lock(Arc::new(MemoryDistributedLock::new()))
        .clock(clock);
    match contexts {
        Some(contexts) => builder.workflow_context_store(contexts),
        None => builder,
    }
    .build()
    .unwrap()
}
async fn contexts(store: Arc<dyn StateStore>) -> Arc<TrustedContextStore> {
    let coordinator =
        AuthorityCoordinator::initialize(store.clone(), NS, TENANT, CoordinatorLimits::default())
            .await
            .unwrap();
    Arc::new(
        TrustedContextStore::new(
            store,
            coordinator,
            "city-domain".into(),
            "k1".into(),
            vec![ContextSigningKey::new("k1".into(), vec![7; 32]).unwrap()],
        )
        .unwrap(),
    )
}
async fn fixture() -> (
    Arc<dyn StateStore>,
    Arc<ManualClock>,
    Arc<TrustedContextStore>,
    Gateway,
    VerifiedExecutionContext,
) {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let clock = Arc::new(ManualClock::new(
        DateTime::<Utc>::from_timestamp_millis(1000).unwrap(),
    ));
    let contexts = contexts(store.clone()).await;
    let coordinator = AuthorityCoordinator::connect(store.clone(), NS, TENANT)
        .await
        .unwrap();
    let context = contexts
        .capture_root(
            RootContextAdmission {
                handle: ExecutionContextHandle::new(),
                binding: ContextBinding {
                    execution_id: Uuid::new_v4(),
                    principal: PrincipalIdentity::new("investigator", PrincipalKind::Agent)
                        .unwrap(),
                    request_digest: workflow_context_digest(
                        NS,
                        TENANT,
                        "diagnose",
                        "diagnostic",
                        &serde_json::json!({"incident":7}),
                    )
                    .unwrap(),
                },
                credential_id: "original-key".into(),
                auth_method: "api_key".into(),
                accepted_ceiling_revision: "ceiling-v1".into(),
                accepted_effects: vec![
                    workflow_context_effect(NS, TENANT, "diagnose", "diagnostic").unwrap(),
                ],
                deadline_ms: 10_000,
                evaluated_authority: coordinator.snapshot().await.unwrap().stamp(),
            },
            1000,
        )
        .await
        .unwrap();
    let gateway = gateway(store.clone(), clock.clone(), Some(contexts.clone()));
    (store, clock, contexts, gateway, context)
}
async fn start(
    gateway: &Gateway,
    context: &VerifiedExecutionContext,
) -> acteon_core::WorkflowExecution {
    gateway
        .start_workflow_with_context(
            NS,
            TENANT,
            "diagnose",
            "diagnostic",
            serde_json::json!({"incident":7}),
            HashMap::new(),
            context,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn restart_and_timer_continuations_retain_verified_original_actor() {
    let (store, clock, _, original, context) = fixture().await;
    let exec = start(&original, &context).await;
    assert_eq!(
        exec.execution_context.as_ref().unwrap().principal().id(),
        "investigator"
    );
    let leased = original
        .poll_worker_tasks(NS, TENANT, "diagnostic", 1, None, Some("worker-one"))
        .await
        .unwrap();
    assert_eq!(leased.len(), 1);
    let task = &leased[0];
    assert_eq!(task.execution_context, exec.execution_context);
    original
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::Sleep {
                checkpoint: "wait".into(),
                seconds: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        original
            .get_workflow_execution(NS, TENANT, &exec.execution_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::WaitingTimer
    );
    let replacement = gateway(store.clone(), clock.clone(), Some(contexts(store).await));
    clock.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(replacement.process_due_workflow_timers().await.unwrap(), 1);
    let next = replacement
        .poll_worker_tasks(
            NS,
            TENANT,
            "diagnostic",
            1,
            None,
            Some("replacement-worker"),
        )
        .await
        .unwrap();
    assert_eq!(next.len(), 1);
    assert_ne!(next[0].task_id, task.task_id);
    assert_eq!(next[0].execution_context, exec.execution_context);
    let recovered = contexts_from_reference(&replacement, &context).await;
    assert_eq!(recovered, "investigator");
}
// Inspection remains available independently of a task's delivery permission.
async fn contexts_from_reference(gateway: &Gateway, context: &VerifiedExecutionContext) -> String {
    gateway
        .get_workflow_execution(NS, TENANT, &context.execution_id().to_string())
        .await
        .unwrap()
        .unwrap()
        .execution_context
        .unwrap()
        .principal()
        .id()
        .into()
}

#[tokio::test]
async fn missing_provenance_and_changed_input_queue_or_target_are_refused() {
    let (_, _, _, gateway, context) = fixture().await;
    assert!(
        gateway
            .start_workflow(
                NS,
                TENANT,
                "diagnose",
                "diagnostic",
                serde_json::json!({"incident":7}),
                HashMap::new()
            )
            .await
            .is_err()
    );
    for (namespace, tenant, workflow, queue, input) in [
        (
            NS,
            TENANT,
            "diagnose",
            "production",
            serde_json::json!({"incident":7}),
        ),
        (
            NS,
            TENANT,
            "remediate",
            "diagnostic",
            serde_json::json!({"incident":7}),
        ),
        (
            NS,
            TENANT,
            "diagnose",
            "diagnostic",
            serde_json::json!({"incident":8}),
        ),
        (
            NS,
            "tenant.prod",
            "diagnose",
            "diagnostic",
            serde_json::json!({"incident":7}),
        ),
    ] {
        assert!(
            gateway
                .start_workflow_with_context(
                    namespace,
                    tenant,
                    workflow,
                    queue,
                    input,
                    HashMap::new(),
                    &context
                )
                .await
                .is_err()
        );
    }
    assert!(
        gateway
            .get_workflow_execution(NS, TENANT, &context.execution_id().to_string())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn repeated_root_cannot_reset_an_existing_workflow() {
    let (_, _, _, gateway, context) = fixture().await;
    let first = start(&gateway, &context).await;
    assert!(
        gateway
            .start_workflow_with_context(
                NS,
                TENANT,
                "diagnose",
                "diagnostic",
                serde_json::json!({"incident":7}),
                HashMap::new(),
                &context
            )
            .await
            .is_err()
    );
    let observed = gateway
        .get_workflow_execution(NS, TENANT, &first.execution_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.current_task_id, first.current_task_id);
    assert_eq!(
        gateway
            .poll_worker_tasks(NS, TENANT, "diagnostic", 10, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn corrupt_or_stripped_task_reference_is_retained_without_leasing() {
    for stripped in [false, true] {
        let (store, _, _, gateway, context) = fixture().await;
        let exec = start(&gateway, &context).await;
        let id = exec.current_task_id.unwrap();
        let key = StateKey::new(NS, TENANT, KeyKind::Custom("worker_task".into()), &id);
        let raw = store.get(&key).await.unwrap().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        if stripped {
            value.as_object_mut().unwrap().remove("execution_context");
        } else {
            value["execution_context"]["principal"]["id"] = "other-agent".into();
        }
        store.set(&key, &value.to_string(), None).await.unwrap();
        assert!(
            gateway
                .poll_worker_tasks(NS, TENANT, "diagnostic", 1, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        let parked = gateway
            .get_worker_task(NS, TENANT, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(parked.status, WorkerTaskStatus::Pending);
        assert_eq!(parked.attempt, 0);
        assert!(parked.lease_token.is_none());
    }
}

#[tokio::test]
async fn workflow_input_corruption_and_expired_context_block_delivery_but_allow_observation() {
    for expired in [false, true] {
        let (store, clock, _, gateway, context) = fixture().await;
        let exec = start(&gateway, &context).await;
        if expired {
            clock.advance_to(Duration::from_secs(10)).unwrap();
        } else {
            let key = StateKey::new(
                NS,
                TENANT,
                KeyKind::Custom("workflow_exec".into()),
                &exec.execution_id,
            );
            let mut value: serde_json::Value =
                serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
            value["input"]["incident"] = 8.into();
            store.set(&key, &value.to_string(), None).await.unwrap();
        }
        assert!(
            gateway
                .poll_worker_tasks(NS, TENANT, "diagnostic", 1, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            gateway
                .get_workflow_execution(NS, TENANT, &exec.execution_id)
                .await
                .unwrap()
                .is_some()
        );
        gateway
            .cancel_workflow(NS, TENANT, &exec.execution_id, Some("operator stop".into()))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn child_creation_refuses_provenance_loss_and_unconfigured_replica_cannot_deliver() {
    let (store, clock, _, gateway_with_context, context) = fixture().await;
    let exec = start(&gateway_with_context, &context).await;
    assert!(
        gateway_with_context
            .start_child_workflow(
                NS,
                TENANT,
                &exec.execution_id,
                "child",
                "remediate",
                None,
                serde_json::json!({}),
                ParentClosePolicy::Abandon
            )
            .await
            .is_err()
    );
    let unchanged = gateway_with_context
        .get_workflow_execution(NS, TENANT, &exec.execution_id)
        .await
        .unwrap()
        .unwrap();
    assert!(unchanged.children.is_empty());
    assert!(unchanged.checkpoint("child").is_none());
    let unconfigured = gateway(store, clock, None);
    assert!(
        unconfigured
            .poll_worker_tasks(NS, TENANT, "diagnostic", 1, None, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn semantic_digest_is_order_independent_and_binds_complete_input() {
    let a = serde_json::json!({"a":1,"b":{"x":2,"y":3}});
    let b: serde_json::Value = serde_json::from_str("{\"b\":{\"y\":3,\"x\":2},\"a\":1}").unwrap();
    assert_eq!(
        workflow_context_digest(NS, TENANT, "wf", "q", &a).unwrap(),
        workflow_context_digest(NS, TENANT, "wf", "q", &b).unwrap()
    );
    assert_ne!(
        workflow_context_digest(NS, TENANT, "wf", "q", &a).unwrap(),
        workflow_context_digest(NS, TENANT, "wf", "other", &a).unwrap()
    );
}

#[tokio::test]
async fn interrupted_publication_repairs_the_same_context_bearing_continuation() {
    for (kind, operation) in [
        ("worker_task", WriteOperation::CheckAndSet),
        ("queue_pending", WriteOperation::Set),
    ] {
        let (store, clock, contexts, _, context) = fixture().await;
        let faults = Arc::new(FaultStore::new(store.clone()));
        faults
            .fail_next(KeyKind::Custom(kind.into()), operation, FaultTiming::Before)
            .unwrap();
        let interrupted = gateway(faults, clock.clone(), Some(contexts.clone()));
        assert!(
            interrupted
                .start_workflow_with_context(
                    NS,
                    TENANT,
                    "diagnose",
                    "diagnostic",
                    serde_json::json!({"incident":7}),
                    HashMap::new(),
                    &context
                )
                .await
                .is_err()
        );
        let durable = interrupted
            .get_workflow_execution(NS, TENANT, &context.execution_id().to_string())
            .await
            .unwrap()
            .unwrap();
        let expected_id = durable.current_task_id.unwrap();
        let replacement = gateway(store, clock, Some(contexts));
        assert_eq!(replacement.reconcile_workflow_discovery().await.unwrap(), 1);
        let tasks = replacement
            .poll_worker_tasks(NS, TENANT, "diagnostic", 1, None, None)
            .await
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, expected_id);
        assert_eq!(tasks[0].execution_context, durable.execution_context);
    }
}
