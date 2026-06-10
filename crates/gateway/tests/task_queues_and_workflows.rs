//! Integration tests for the worker task queue (Phase 2) and the
//! checkpoint-based workflow engine (Phase 3).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use acteon_core::chain::{ChainConfig, ChainStepConfig, WorkerStepConfig};
use acteon_core::{
    Action, ActionOutcome, ChainStatus, ExecutionEventType, ParentClosePolicy, ProviderResponse,
    WORKFLOW_TASK_ACTION_TYPE, WorkerTask, WorkerTaskStatus, WorkflowDirective, WorkflowStatus,
};
use acteon_executor::ExecutorConfig;
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::ir::expr::{BinaryOp, Expr};
use acteon_rules::ir::rule::{Rule, RuleAction};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

const NS: &str = "notifications";
const TENANT: &str = "tenant-1";

struct MockProvider;

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        "email"
    }

    async fn execute(&self, _action: &Action) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::success(serde_json::json!({"ok": true})))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn build_gateway(chains: Vec<ChainConfig>) -> Gateway {
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
    let mut builder = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(vec![rule])
        .provider(Arc::new(MockProvider))
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

// -- Task queue lifecycle ------------------------------------------------------

#[tokio::test]
async fn task_lifecycle_enqueue_poll_complete() {
    let gateway = build_gateway(vec![]);
    let task = WorkerTask::new(NS, TENANT, "builds", "compile", serde_json::json!({"n": 1}));
    let task_id = task.task_id.clone();
    gateway.enqueue_worker_task(task).await.unwrap();

    // Poll leases the task with a lease token.
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "builds", 10, Some(60), Some("worker-a"))
        .await
        .unwrap();
    assert_eq!(leased.len(), 1);
    let lease = leased[0].lease_token.clone().unwrap();
    assert_eq!(leased[0].status, WorkerTaskStatus::Leased);
    assert_eq!(leased[0].attempt, 1);

    // A second poll returns nothing while the lease is held.
    let again = gateway
        .poll_worker_tasks(NS, TENANT, "builds", 10, Some(60), Some("worker-b"))
        .await
        .unwrap();
    assert!(again.is_empty());

    // Heartbeat extends; complete settles.
    gateway
        .heartbeat_worker_task(NS, TENANT, &task_id, &lease, Some(120))
        .await
        .unwrap();
    let done = gateway
        .complete_worker_task(NS, TENANT, &task_id, &lease, serde_json::json!({"out": 42}))
        .await
        .unwrap();
    assert_eq!(done.status, WorkerTaskStatus::Completed);
    assert_eq!(done.result, Some(serde_json::json!({"out": 42})));

    // Completing again with the stale lease errors.
    let err = gateway
        .complete_worker_task(NS, TENANT, &task_id, &lease, serde_json::Value::Null)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not leased"));
}

#[tokio::test]
async fn retryable_failure_requeues_with_backoff_then_fails_terminally() {
    let gateway = build_gateway(vec![]);
    let task = WorkerTask::new(NS, TENANT, "q", "a", serde_json::json!({})).with_max_attempts(2);
    let task_id = task.task_id.clone();
    gateway.enqueue_worker_task(task).await.unwrap();

    // Attempt 1 fails retryably → re-queued with not_before backoff.
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(60), None)
        .await
        .unwrap();
    let lease = leased[0].lease_token.clone().unwrap();
    let after_fail = gateway
        .fail_worker_task(NS, TENANT, &task_id, &lease, "boom", true)
        .await
        .unwrap();
    assert_eq!(after_fail.status, WorkerTaskStatus::Pending);
    assert!(after_fail.not_before.is_some());

    // Not leasable until the backoff elapses.
    let early = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(60), None)
        .await
        .unwrap();
    assert!(early.is_empty(), "task should be backing off");

    tokio::time::sleep(Duration::from_millis(2100)).await;
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(60), None)
        .await
        .unwrap();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].attempt, 2);
    let lease = leased[0].lease_token.clone().unwrap();

    // Attempt 2 fails: budget exhausted → terminal.
    let terminal = gateway
        .fail_worker_task(NS, TENANT, &task_id, &lease, "boom again", true)
        .await
        .unwrap();
    assert_eq!(terminal.status, WorkerTaskStatus::Failed);
}

#[tokio::test]
async fn expired_lease_is_reclaimed_on_next_poll() {
    let gateway = build_gateway(vec![]);
    let task = WorkerTask::new(NS, TENANT, "q", "a", serde_json::json!({}));
    let task_id = task.task_id.clone();
    gateway.enqueue_worker_task(task).await.unwrap();

    // Lease with the minimum duration and let it expire.
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(1), Some("crashed-worker"))
        .await
        .unwrap();
    assert_eq!(leased.len(), 1);
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // The reclaim runs on poll; backoff for attempt 1 is ~2s, so wait it out.
    let nothing = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(60), Some("worker-b"))
        .await
        .unwrap();
    assert!(nothing.is_empty());
    tokio::time::sleep(Duration::from_millis(2100)).await;

    let released = gateway
        .poll_worker_tasks(NS, TENANT, "q", 1, Some(60), Some("worker-b"))
        .await
        .unwrap();
    assert_eq!(released.len(), 1, "expired lease should be re-delivered");
    assert_eq!(released[0].task_id, task_id);
    assert_eq!(released[0].attempt, 2);
}

// -- Worker chain steps --------------------------------------------------------

fn worker_chain() -> ChainConfig {
    ChainConfig::new("test-chain")
        .with_step(ChainStepConfig::new_worker(
            "build",
            WorkerStepConfig {
                queue: "builds".into(),
                action_type: Some("compile".into()),
                timeout_seconds: None,
                max_attempts: Some(1),
            },
            serde_json::json!({"req": "{{origin.payload.request}}"}),
        ))
        .with_step(ChainStepConfig::new(
            "notify",
            "email",
            "send_email",
            serde_json::json!({"result": "{{prev.body.artifact}}"}),
        ))
}

async fn start_chain(gateway: &Gateway) -> String {
    let action = Action::new(
        NS,
        TENANT,
        "email",
        "start_chain",
        serde_json::json!({"request": "build-42"}),
    );
    match gateway.dispatch(action, None).await.unwrap() {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id,
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

#[tokio::test]
async fn worker_step_enqueues_task_and_resumes_chain_on_completion() {
    let gateway = build_gateway(vec![worker_chain()]);
    let chain_id = start_chain(&gateway).await;

    // Advancing the chain enqueues the worker task and pauses.
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingWorker);

    // The worker polls, sees the templated payload, and completes.
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "builds", 1, Some(60), Some("ci-worker"))
        .await
        .unwrap();
    assert_eq!(leased.len(), 1);
    let task = &leased[0];
    assert_eq!(task.action_type, "compile");
    assert_eq!(task.payload, serde_json::json!({"req": "build-42"}));
    assert_eq!(task.chain_id.as_deref(), Some(chain_id.as_str()));

    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::json!({"artifact": "app-v1.tgz"}),
        )
        .await
        .unwrap();

    // Completion resumed the chain to the next step; drive it to the end.
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();
    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Completed);
    assert_eq!(
        state.step_results[0].as_ref().unwrap().response_body,
        Some(serde_json::json!({"artifact": "app-v1.tgz"}))
    );

    // History records the enqueue/complete pair.
    let history = gateway
        .get_execution_history(NS, TENANT, &chain_id)
        .await
        .unwrap();
    assert!(history.events.iter().any(|e| matches!(
        &e.event,
        ExecutionEventType::TaskEnqueued { queue, .. } if queue == "builds"
    )));
    assert!(
        history
            .events
            .iter()
            .any(|e| matches!(&e.event, ExecutionEventType::TaskCompleted { .. }))
    );
}

#[tokio::test]
async fn worker_step_terminal_failure_fails_chain() {
    let gateway = build_gateway(vec![worker_chain()]);
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();

    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "builds", 1, Some(60), None)
        .await
        .unwrap();
    let task = &leased[0];
    gateway
        .fail_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            "compiler exploded",
            false,
        )
        .await
        .unwrap();

    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::Failed);
    assert_eq!(
        state.step_results[0].as_ref().unwrap().error.as_deref(),
        Some("compiler exploded")
    );
}

// -- Workflow engine -----------------------------------------------------------

/// Poll the workflow queue and return the single continuation task.
async fn poll_workflow_task(gateway: &Gateway, queue: &str) -> WorkerTask {
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, queue, 1, Some(60), Some("wf-worker"))
        .await
        .unwrap();
    assert_eq!(leased.len(), 1, "expected one workflow continuation task");
    assert_eq!(leased[0].action_type, WORKFLOW_TASK_ACTION_TYPE);
    leased[0].clone()
}

/// Settle a continuation task with a directive, as the worker SDK would.
async fn settle(gateway: &Gateway, task: &WorkerTask, directive: WorkflowDirective) {
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(&directive).unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn workflow_checkpoints_sleep_and_complete() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(
            NS,
            TENANT,
            "order-flow",
            "wf-queue",
            serde_json::json!({"order": 7}),
            HashMap::new(),
        )
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    // First continuation: worker records a step checkpoint, then sleeps.
    let task = poll_workflow_task(&gateway, "wf-queue").await;
    assert_eq!(task.payload["input"], serde_json::json!({"order": 7}));
    assert_eq!(task.payload["checkpoints"], serde_json::json!([]));
    gateway
        .record_workflow_checkpoint(
            NS,
            TENANT,
            &id,
            "step:charge#1",
            serde_json::json!({"charged": true}),
        )
        .await
        .unwrap();
    settle(
        &gateway,
        &task,
        WorkflowDirective::Sleep {
            checkpoint: "sleep:1".into(),
            seconds: 1,
        },
    )
    .await;

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::WaitingTimer);

    // Timer fires (driven by the background tick in production).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let fired = gateway.process_due_workflow_timers().await.unwrap();
    assert_eq!(fired, 1);

    // Second continuation: checkpoints are replayed in the snapshot.
    let task = poll_workflow_task(&gateway, "wf-queue").await;
    let checkpoint_names: Vec<&str> = task.payload["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(checkpoint_names, vec!["step:charge#1", "sleep:1"]);

    settle(
        &gateway,
        &task,
        WorkflowDirective::Complete {
            result: serde_json::json!({"done": true}),
        },
    )
    .await;

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::Completed);
    assert_eq!(exec.result, Some(serde_json::json!({"done": true})));

    // History shows the full lifecycle.
    let history = gateway
        .get_execution_history(NS, TENANT, &id)
        .await
        .unwrap();
    assert!(
        history
            .events
            .iter()
            .any(|e| matches!(&e.event, ExecutionEventType::TimerFired { .. }))
    );
    assert!(
        history
            .events
            .iter()
            .any(|e| matches!(&e.event, ExecutionEventType::ExecutionCompleted))
    );
}

#[tokio::test]
async fn workflow_await_signal_resumes_with_payload() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(
            NS,
            TENANT,
            "approval-flow",
            "wf-q",
            serde_json::json!({}),
            HashMap::new(),
        )
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    let task = poll_workflow_task(&gateway, "wf-q").await;
    settle(
        &gateway,
        &task,
        WorkflowDirective::AwaitSignal {
            checkpoint: "signal:approved#1".into(),
            name: "approved".into(),
            timeout_seconds: None,
        },
    )
    .await;

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::WaitingSignal);

    gateway
        .signal_workflow(
            NS,
            TENANT,
            &id,
            "approved",
            serde_json::json!({"by": "renzo"}),
        )
        .await
        .unwrap();

    // Resumed: the signal payload is the recorded checkpoint.
    let task = poll_workflow_task(&gateway, "wf-q").await;
    let checkpoints = task.payload["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints[0]["name"], "signal:approved#1");
    assert_eq!(checkpoints[0]["data"], serde_json::json!({"by": "renzo"}));

    settle(
        &gateway,
        &task,
        WorkflowDirective::Complete {
            result: serde_json::Value::Null,
        },
    )
    .await;
    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::Completed);
}

#[tokio::test]
async fn workflow_buffered_signal_satisfies_later_await() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(NS, TENANT, "wf", "q", serde_json::json!({}), HashMap::new())
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    // Signal arrives while the first continuation is still leased.
    let task = poll_workflow_task(&gateway, "q").await;
    gateway
        .signal_workflow(NS, TENANT, &id, "go", serde_json::json!("early"))
        .await
        .unwrap();

    // The await consumes the buffered signal and re-enqueues immediately.
    settle(
        &gateway,
        &task,
        WorkflowDirective::AwaitSignal {
            checkpoint: "signal:go#1".into(),
            name: "go".into(),
            timeout_seconds: None,
        },
    )
    .await;

    let task = poll_workflow_task(&gateway, "q").await;
    let checkpoints = task.payload["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints[0]["data"], serde_json::json!("early"));
    settle(
        &gateway,
        &task,
        WorkflowDirective::Complete {
            result: serde_json::Value::Null,
        },
    )
    .await;
}

#[tokio::test]
async fn workflow_signal_timeout_records_timed_out_checkpoint() {
    let gateway = build_gateway(vec![]);
    gateway
        .start_workflow(NS, TENANT, "wf", "q", serde_json::json!({}), HashMap::new())
        .await
        .unwrap();

    let task = poll_workflow_task(&gateway, "q").await;
    settle(
        &gateway,
        &task,
        WorkflowDirective::AwaitSignal {
            checkpoint: "signal:never#1".into(),
            name: "never".into(),
            timeout_seconds: Some(1),
        },
    )
    .await;

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let fired = gateway.process_due_workflow_timers().await.unwrap();
    assert_eq!(fired, 1);

    let task = poll_workflow_task(&gateway, "q").await;
    let checkpoints = task.payload["checkpoints"].as_array().unwrap();
    assert_eq!(
        checkpoints[0]["data"],
        serde_json::json!({"timed_out": true})
    );
    settle(
        &gateway,
        &task,
        WorkflowDirective::Complete {
            result: serde_json::Value::Null,
        },
    )
    .await;
}

#[tokio::test]
async fn child_workflow_result_signals_parent() {
    let gateway = build_gateway(vec![]);
    let parent = gateway
        .start_workflow(
            NS,
            TENANT,
            "parent",
            "q",
            serde_json::json!({}),
            HashMap::new(),
        )
        .await
        .unwrap();
    let parent_id = parent.execution_id.clone();

    // Parent's first continuation starts a child, then awaits its result.
    let parent_task = poll_workflow_task(&gateway, "q").await;
    let child_id = gateway
        .start_child_workflow(
            NS,
            TENANT,
            &parent_id,
            "child:sub#1",
            "child-flow",
            Some("child-q"),
            serde_json::json!({"part": 1}),
            ParentClosePolicy::Abandon,
        )
        .await
        .unwrap();
    // Idempotent replay returns the same child.
    let replay_id = gateway
        .start_child_workflow(
            NS,
            TENANT,
            &parent_id,
            "child:sub#1",
            "child-flow",
            Some("child-q"),
            serde_json::json!({"part": 1}),
            ParentClosePolicy::Abandon,
        )
        .await
        .unwrap();
    assert_eq!(child_id, replay_id);

    settle(
        &gateway,
        &parent_task,
        WorkflowDirective::AwaitSignal {
            checkpoint: format!("signal:__child:{child_id}#1"),
            name: format!("__child:{child_id}"),
            timeout_seconds: None,
        },
    )
    .await;

    // The child runs and completes; its result is signalled to the parent.
    let child_task = poll_workflow_task(&gateway, "child-q").await;
    settle(
        &gateway,
        &child_task,
        WorkflowDirective::Complete {
            result: serde_json::json!({"part_done": 1}),
        },
    )
    .await;

    // Parent resumed with the child result in the checkpoint.
    let parent_task = poll_workflow_task(&gateway, "q").await;
    let checkpoints = parent_task.payload["checkpoints"].as_array().unwrap();
    let child_checkpoint = checkpoints
        .iter()
        .find(|c| c["name"].as_str().unwrap().starts_with("signal:__child:"))
        .unwrap();
    assert_eq!(child_checkpoint["data"]["status"], "completed");
    assert_eq!(
        child_checkpoint["data"]["result"],
        serde_json::json!({"part_done": 1})
    );

    settle(
        &gateway,
        &parent_task,
        WorkflowDirective::Complete {
            result: serde_json::Value::Null,
        },
    )
    .await;
    let parent = gateway
        .get_workflow_execution(NS, TENANT, &parent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.status, WorkflowStatus::Completed);
}

#[tokio::test]
async fn parent_close_cancels_children_with_cancel_policy() {
    let gateway = build_gateway(vec![]);
    let parent = gateway
        .start_workflow(
            NS,
            TENANT,
            "parent",
            "q",
            serde_json::json!({}),
            HashMap::new(),
        )
        .await
        .unwrap();
    let parent_id = parent.execution_id.clone();

    let parent_task = poll_workflow_task(&gateway, "q").await;
    let child_id = gateway
        .start_child_workflow(
            NS,
            TENANT,
            &parent_id,
            "child:bg#1",
            "bg-flow",
            Some("child-q"),
            serde_json::json!({}),
            ParentClosePolicy::Cancel,
        )
        .await
        .unwrap();

    // Parent completes without waiting for the child.
    settle(
        &gateway,
        &parent_task,
        WorkflowDirective::Complete {
            result: serde_json::Value::Null,
        },
    )
    .await;

    let child = gateway
        .get_workflow_execution(NS, TENANT, &child_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.status, WorkflowStatus::Cancelled);
}

#[tokio::test]
async fn workflow_task_terminal_failure_fails_execution() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(NS, TENANT, "wf", "q", serde_json::json!({}), HashMap::new())
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    let task = poll_workflow_task(&gateway, "q").await;
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::Fail {
                error: "unrecoverable".into(),
            })
            .unwrap(),
        )
        .await
        .unwrap();

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::Failed);
    assert_eq!(exec.error.as_deref(), Some("unrecoverable"));
}

#[tokio::test]
async fn workflow_cancellation() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(NS, TENANT, "wf", "q", serde_json::json!({}), HashMap::new())
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    let cancelled = gateway
        .cancel_workflow(NS, TENANT, &id, Some("operator".into()))
        .await
        .unwrap();
    assert_eq!(cancelled.status, WorkflowStatus::Cancelled);

    // Signals to a cancelled execution are rejected.
    let err = gateway
        .signal_workflow(NS, TENANT, &id, "x", serde_json::Value::Null)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not active"));
}

// -- Adversarial-review regression tests ---------------------------------------

/// Blocker 3: a present-but-malformed directive (e.g. float seconds) must
/// fail the execution loudly, never silently complete it.
#[tokio::test]
async fn malformed_directive_fails_execution_instead_of_completing() {
    let gateway = build_gateway(vec![]);
    let exec = gateway
        .start_workflow(NS, TENANT, "wf", "q", serde_json::json!({}), HashMap::new())
        .await
        .unwrap();
    let id = exec.execution_id.clone();

    let task = poll_workflow_task(&gateway, "q").await;
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::json!({"directive": "sleep", "checkpoint": "sleep#0", "seconds": 1.5}),
        )
        .await
        .unwrap();

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::Failed);
    assert!(
        exec.error
            .as_deref()
            .unwrap_or_default()
            .contains("malformed"),
        "error should name the malformed directive, got {:?}",
        exec.error
    );
}

/// Blocker 4: cancelling a chain parked on a worker step must cancel the
/// outstanding task so the work cannot execute afterwards.
#[tokio::test]
async fn cancel_chain_cancels_outstanding_worker_task() {
    let gateway = build_gateway(vec![worker_chain()]);
    let chain_id = start_chain(&gateway).await;
    gateway.advance_chain(NS, TENANT, &chain_id).await.unwrap();

    let state = gateway
        .get_chain_status(NS, TENANT, &chain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, ChainStatus::WaitingWorker);
    let acteon_core::chain::WaitState::Worker { task_id, .. } = state.wait_state.clone().unwrap()
    else {
        panic!("expected worker wait state");
    };

    gateway
        .cancel_chain(NS, TENANT, &chain_id, Some("stop".into()), None)
        .await
        .unwrap();

    let task = gateway
        .get_worker_task(NS, TENANT, &task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.status, WorkerTaskStatus::Cancelled);
    // No worker can lease it anymore.
    let leased = gateway
        .poll_worker_tasks(NS, TENANT, "builds", 10, Some(60), None)
        .await
        .unwrap();
    assert!(leased.is_empty());
}

/// Finding 9a: queue names containing the key delimiter are rejected so
/// per-queue prefix scans cannot cross-contaminate.
#[tokio::test]
async fn queue_names_with_delimiter_are_rejected() {
    let gateway = build_gateway(vec![]);
    let err = gateway
        .enqueue_worker_task(WorkerTask::new(
            NS,
            TENANT,
            "etl:high",
            "a",
            serde_json::json!({}),
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid queue name"));

    let err = gateway
        .poll_worker_tasks(NS, TENANT, "etl:high", 1, None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid queue name"));
}
