use futures::poll;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use acteon_core::{Action, WorkerTask, WorkflowStatus};
use acteon_executor::{DeadLetterEntry, DeadLetterError, DeadLetterQueue, DeadLetterSink};
use acteon_gateway::{BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder};
use acteon_state::{
    KeyKind, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use serde_json::json;

fn kind(value: &str) -> KeyKind {
    KeyKind::Custom(value.into())
}
struct Sink {
    unavailable: AtomicBool,
    attempts: AtomicUsize,
    queue: DeadLetterQueue,
    seen: Mutex<HashSet<String>>,
    ack_fault: Mutex<Option<(Arc<FaultStore>, FaultTiming)>>,
}
impl Sink {
    fn new() -> Self {
        Self {
            unavailable: AtomicBool::new(false),
            attempts: AtomicUsize::new(0),
            queue: DeadLetterQueue::new(),
            seen: Mutex::new(HashSet::new()),
            ack_fault: Mutex::new(None),
        }
    }
}
#[async_trait::async_trait]
impl DeadLetterSink for Sink {
    async fn push(
        &self,
        action: Action,
        error: String,
        attempts: u32,
    ) -> Result<(), DeadLetterError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(DeadLetterError("injected sink outage".into()));
        }
        if self.seen.lock().unwrap().insert(action.id.to_string()) {
            self.queue.push(action, error, attempts);
        }
        if let Some((fault, timing)) = self.ack_fault.lock().unwrap().take() {
            fault
                .fail_next(kind("worker_task"), WriteOperation::CompareAndSwap, timing)
                .unwrap();
        }
        Ok(())
    }
    async fn drain(&self) -> Vec<DeadLetterEntry> {
        self.queue.drain()
    }
    async fn len(&self) -> usize {
        self.queue.len()
    }
}
struct Fixture {
    clock: Arc<ManualClock>,
    store: Arc<MemoryStateStore>,
    fault: Arc<FaultStore>,
    lock: Arc<MemoryDistributedLock>,
    sink: Arc<Sink>,
    encrypted: bool,
}
impl Fixture {
    fn new() -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        Self {
            fault: Arc::new(FaultStore::new(store.clone())),
            lock: Arc::new(MemoryDistributedLock::with_clock(clock.clone())),
            clock,
            store,
            sink: Arc::new(Sink::new()),
            encrypted: false,
        }
    }
    fn gateway(&self) -> Gateway {
        let rule = acteon_rules::ir::rule::Rule::new(
            "chain",
            acteon_rules::ir::expr::Expr::Bool(true),
            acteon_rules::ir::rule::RuleAction::Chain {
                chain: "handoff".into(),
            },
        );
        let chain = acteon_core::chain::ChainConfig::new("handoff")
            .with_step(acteon_core::chain::ChainStepConfig::new_worker(
                "first",
                acteon_core::chain::WorkerStepConfig {
                    queue: "work".into(),
                    action_type: Some("job".into()),
                    timeout_seconds: None,
                    max_attempts: Some(1),
                },
                json!({}),
            ))
            .with_step(acteon_core::chain::ChainStepConfig::new_worker(
                "second",
                acteon_core::chain::WorkerStepConfig {
                    queue: "next".into(),
                    action_type: Some("next".into()),
                    timeout_seconds: None,
                    max_attempts: Some(1),
                },
                json!({}),
            ));
        let mut builder = GatewayBuilder::new()
            .state(self.fault.clone())
            .lock(self.lock.clone())
            .clock(self.clock.clone())
            .dlq_sink(self.sink.clone())
            .chain(chain)
            .rules(vec![rule]);
        if self.encrypted {
            builder = builder.payload_encryptor(Arc::new(acteon_crypto::PayloadEncryptor::new(
                acteon_crypto::parse_master_key(&"45".repeat(32)).unwrap(),
            )));
        }
        builder.build().unwrap()
    }
    async fn chain(&self, gateway: &Gateway) -> (String, WorkerTask) {
        let outcome = gateway
            .dispatch(
                Action::new("ns", "tenant", "worker", "chain", json!({})),
                None,
            )
            .await
            .unwrap();
        let acteon_core::ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
            panic!("chain did not start");
        };
        gateway
            .advance_chain("ns", "tenant", &chain_id)
            .await
            .unwrap();
        let task = gateway
            .poll_worker_tasks("ns", "tenant", "work", 1, Some(60), None)
            .await
            .unwrap()
            .remove(0);
        (chain_id, task)
    }
    async fn task(&self, id: &str) -> WorkerTask {
        self.gateway()
            .get_worker_task("ns", "tenant", id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn cleanup(&self) {
        let gateway = Arc::new(tokio::sync::RwLock::new(self.gateway()));
        let (mut worker, _) = BackgroundProcessorBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .group_manager(gateway.read().await.group_manager())
            .metrics(gateway.read().await.metrics_arc())
            .gateway(gateway.clone())
            .build()
            .unwrap();
        worker.tick(BackgroundJob::Cleanup).await.unwrap();
    }
    async fn workflow(&self, gateway: &Gateway) -> (String, WorkerTask) {
        let exec = gateway
            .start_workflow("ns", "tenant", "flow", "work", json!({}), HashMap::new())
            .await
            .unwrap();
        let task = gateway
            .poll_worker_tasks("ns", "tenant", "work", 1, Some(60), None)
            .await
            .unwrap()
            .remove(0);
        (exec.execution_id, task)
    }
}
#[tokio::test]
async fn lost_terminal_write_ack_does_not_lose_workflow_handoff() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        gateway
            .complete_worker_task(
                "ns",
                "tenant",
                &task.task_id,
                task.lease_token.as_deref().unwrap(),
                json!({"done":true})
            )
            .await
            .is_err()
    );
    drop(gateway);
    f.cleanup().await;
    assert_eq!(
        f.gateway()
            .get_workflow_execution("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Completed
    );
}
#[tokio::test]
async fn workflow_storage_outage_is_retried_after_restart() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            json!({"done":true}),
        )
        .await
        .unwrap();
    drop(gateway);
    f.cleanup().await;
    assert_eq!(
        f.gateway()
            .get_workflow_execution("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Completed
    );
}
#[tokio::test]
async fn dlq_outage_keeps_terminal_task_beyond_normal_retention() {
    let f = Fixture::new();
    let gateway = f.gateway();
    f.sink.unavailable.store(true, Ordering::SeqCst);
    gateway
        .enqueue_worker_task(WorkerTask::new_at(
            "ns",
            "tenant",
            "work",
            "job",
            json!({"n":1}),
            f.clock.now(),
        ))
        .await
        .unwrap();
    let task = gateway
        .poll_worker_tasks("ns", "tenant", "work", 1, Some(60), None)
        .await
        .unwrap()
        .remove(0);
    gateway
        .fail_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            "failure",
            false,
        )
        .await
        .unwrap();
    f.clock.advance_to(Duration::from_secs(25 * 3600)).unwrap();
    assert!(
        gateway
            .get_worker_task("ns", "tenant", &task.task_id)
            .await
            .unwrap()
            .is_some()
    );
    f.sink.unavailable.store(false, Ordering::SeqCst);
    drop(gateway);
    f.cleanup().await;
    assert_eq!(f.sink.queue.len(), 1);
    f.cleanup().await;
    assert_eq!(f.sink.queue.len(), 1);
}

#[tokio::test]
async fn chain_result_and_ready_index_survive_each_write_boundary() {
    for (operation, timing) in [
        (WriteOperation::CompareAndSwap, FaultTiming::Before),
        (WriteOperation::CompareAndSwap, FaultTiming::After),
        (WriteOperation::IndexChainReady, FaultTiming::Before),
    ] {
        let f = Fixture::new();
        let gateway = f.gateway();
        let (id, task) = f.chain(&gateway).await;
        let key_kind = if operation == WriteOperation::CompareAndSwap {
            KeyKind::Chain
        } else {
            KeyKind::PendingChains
        };
        f.fault.fail_next(key_kind, operation, timing).unwrap();
        gateway
            .complete_worker_task(
                "ns",
                "tenant",
                &task.task_id,
                task.lease_token.as_deref().unwrap(),
                json!({"ok":1}),
            )
            .await
            .unwrap();
        assert_eq!(f.fault.consumed(), 1);
        drop(gateway);
        f.cleanup().await;
        let gateway = f.gateway();
        let state = gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.current_step, 1);
        assert!(state.step_results[0].as_ref().unwrap().success);
        assert!(
            f.store
                .get_ready_chains(f.clock.now().timestamp_millis())
                .await
                .unwrap()
                .iter()
                .any(|key| key.ends_with(&id))
        );
        f.cleanup().await;
        assert_eq!(
            gateway
                .get_chain_status("ns", "tenant", &id)
                .await
                .unwrap()
                .unwrap()
                .execution_path
                .len(),
            2
        );
        assert!(
            f.task(&task.task_id)
                .await
                .handoff
                .unwrap()
                .completed_at
                .is_some()
        );
    }
}

#[tokio::test]
async fn workflow_timer_index_is_repaired_after_receiver_commit() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    f.fault
        .fail_next(
            kind("workflow_timer"),
            WriteOperation::IndexTimeout,
            FaultTiming::Before,
        )
        .unwrap();
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            json!({"directive":"sleep","checkpoint":"wait","seconds":10}),
        )
        .await
        .unwrap();
    assert_eq!(f.fault.consumed(), 1);
    f.cleanup().await;
    f.clock.advance_to(Duration::from_secs(10)).unwrap();
    assert_eq!(gateway.process_due_workflow_timers().await.unwrap(), 1);
    let exec = gateway
        .get_workflow_execution("ns", "tenant", &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exec.checkpoints.len(), 1);
    assert_eq!(
        gateway
            .poll_worker_tasks("ns", "tenant", "work", 10, Some(60), None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn continuation_creation_failure_preserves_one_published_identity() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    gateway
        .record_workflow_checkpoint("ns", "tenant", &id, "already", json!({"done":1}))
        .await
        .unwrap();
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CheckAndSet,
            FaultTiming::Before,
        )
        .unwrap();
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            json!({"directive":"sleep","checkpoint":"already","seconds":10}),
        )
        .await
        .unwrap();
    let next = gateway
        .get_workflow_execution("ns", "tenant", &id)
        .await
        .unwrap()
        .unwrap()
        .current_task_id
        .unwrap();
    assert_ne!(next, task.task_id);
    assert!(
        gateway
            .get_worker_task("ns", "tenant", &next)
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
    f.cleanup().await;
    let deliveries = gateway
        .poll_worker_tasks("ns", "tenant", "work", 10, Some(60), None)
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].task_id, next);
}

#[tokio::test]
async fn dlq_acknowledgement_interruptions_reuse_identity_and_finalize_progress() {
    for timing in [FaultTiming::Before, FaultTiming::After] {
        let f = Fixture::new();
        let gateway = f.gateway();
        gateway
            .enqueue_worker_task(WorkerTask::new_at(
                "ns",
                "tenant",
                "work",
                "job",
                json!({}),
                f.clock.now(),
            ))
            .await
            .unwrap();
        let task = gateway
            .poll_worker_tasks("ns", "tenant", "work", 1, Some(60), None)
            .await
            .unwrap()
            .remove(0);
        *f.sink.ack_fault.lock().unwrap() = Some((f.fault.clone(), timing));
        gateway
            .fail_worker_task(
                "ns",
                "tenant",
                &task.task_id,
                task.lease_token.as_deref().unwrap(),
                "failed",
                false,
            )
            .await
            .unwrap();
        assert_eq!(f.sink.queue.len(), 1);
        assert!(
            f.task(&task.task_id)
                .await
                .handoff
                .unwrap()
                .completed_at
                .is_none()
        );
        f.clock.advance_to(Duration::from_secs(60)).unwrap();
        f.cleanup().await;
        assert_eq!(
            f.sink.queue.len(),
            1,
            "test sink deduplicates the stable delivery ID"
        );
        assert_eq!(
            f.sink.attempts.load(Ordering::SeqCst),
            if timing == FaultTiming::Before { 2 } else { 1 }
        );
        assert!(
            f.task(&task.task_id)
                .await
                .handoff
                .unwrap()
                .completed_at
                .is_some()
        );
        f.clock
            .advance_to(Duration::from_secs(60 + 24 * 3600))
            .unwrap();
        assert!(
            gateway
                .get_worker_task("ns", "tenant", &task.task_id)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn expired_delivery_owner_cannot_acknowledge_over_its_successor() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (_id, task) = f.chain(&gateway).await;
    let resume = f
        .fault
        .pause_next(
            KeyKind::PendingChains,
            WriteOperation::IndexChainReady,
            FaultTiming::After,
        )
        .unwrap();
    let completion = gateway.complete_worker_task(
        "ns",
        "tenant",
        &task.task_id,
        task.lease_token.as_deref().unwrap(),
        json!({"v":1}),
    );
    let mut completion = Box::pin(completion);
    assert!(poll!(&mut completion).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    let old = f.task(&task.task_id).await.handoff.unwrap().lease_token;
    f.clock.advance_to(Duration::from_secs(60)).unwrap();
    f.cleanup().await;
    let settled = f.task(&task.task_id).await.handoff.unwrap();
    assert!(settled.completed_at.is_some());
    assert_ne!(settled.lease_token, old);
    resume.send(()).unwrap();
    completion.await.unwrap();
    assert_eq!(
        f.task(&task.task_id).await.handoff.unwrap().completed_at,
        settled.completed_at
    );
}

#[tokio::test]
async fn handoff_lease_renews_and_cancellation_leaves_recoverable_progress() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (_id, task) = f.chain(&gateway).await;
    let _resume = f
        .fault
        .pause_next(
            KeyKind::PendingChains,
            WriteOperation::IndexChainReady,
            FaultTiming::After,
        )
        .unwrap();
    let mut completion = Box::pin(gateway.complete_worker_task(
        "ns",
        "tenant",
        &task.task_id,
        task.lease_token.as_deref().unwrap(),
        json!({}),
    ));
    assert!(poll!(&mut completion).is_pending());
    f.clock.advance_to(Duration::from_secs(20)).unwrap();
    assert!(poll!(&mut completion).is_pending());
    assert_eq!(
        f.task(&task.task_id)
            .await
            .handoff
            .unwrap()
            .lease_expires_at,
        Some(f.clock.now() + chrono::Duration::seconds(60))
    );
    assert_eq!(f.gateway().reconcile_worker_handoffs().await.unwrap(), 0);
    drop(completion);
    f.clock.advance_to(Duration::from_secs(80)).unwrap();
    f.cleanup().await;
    assert!(
        f.task(&task.task_id)
            .await
            .handoff
            .unwrap()
            .completed_at
            .is_some()
    );
}

#[tokio::test]
async fn pending_handoff_and_delivery_progress_remain_encrypted() {
    let mut f = Fixture::new();
    f.encrypted = true;
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            json!({"private":"result"}),
        )
        .await
        .unwrap();
    for stage in 0..2 {
        for kind_name in ["worker_task", "workflow_exec"] {
            let rows = f.store.scan_keys_by_kind(kind(kind_name)).await.unwrap();
            assert!(rows.iter().all(|(_, raw)| acteon_crypto::is_encrypted(raw)));
        }
        if stage == 0 {
            f.cleanup().await;
        }
    }
    assert!(
        f.task(&task.task_id)
            .await
            .handoff
            .unwrap()
            .completed_at
            .is_some()
    );
    assert_eq!(
        gateway
            .get_workflow_execution("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .result,
        Some(json!({"private":"result"}))
    );
}

#[tokio::test]
// Keep the two distinct receiver commits and their recovery observations together.
#[allow(clippy::too_many_lines)]
async fn child_result_ack_loss_preserves_one_signal_and_continuation() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (parent, parent_task) = f.workflow(&gateway).await;
    let child = gateway
        .start_child_workflow(
            "ns",
            "tenant",
            &parent,
            "child",
            "child-flow",
            Some("children"),
            json!({}),
            acteon_core::ParentClosePolicy::Abandon,
        )
        .await
        .unwrap();
    let signal = format!("{}{child}", acteon_core::CHILD_RESULT_SIGNAL_PREFIX);
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &parent_task.task_id,
            parent_task.lease_token.as_deref().unwrap(),
            json!({"directive":"await_signal","checkpoint":"child-result","name":signal}),
        )
        .await
        .unwrap();
    let child_task = gateway
        .poll_worker_tasks("ns", "tenant", "children", 1, Some(60), None)
        .await
        .unwrap()
        .remove(0);
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &child_task.task_id,
            child_task.lease_token.as_deref().unwrap(),
            json!({"answer":42}),
        )
        .await
        .unwrap();
    assert!(
        gateway
            .get_workflow_execution("ns", "tenant", &child)
            .await
            .unwrap()
            .unwrap()
            .close_pending
    );
    // Unfinished close effects retain the receiver beyond its normal seven days.
    f.clock
        .advance_to(Duration::from_secs(8 * 24 * 3600))
        .unwrap();
    assert!(
        gateway
            .get_workflow_execution("ns", "tenant", &child)
            .await
            .unwrap()
            .is_some()
    );
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(gateway.reconcile_worker_handoffs().await.is_err());
    let applied = gateway
        .get_workflow_execution("ns", "tenant", &parent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(applied.received_signal_ids.len(), 1);
    let next = applied.current_task_id.unwrap();
    assert!(
        gateway
            .get_worker_task("ns", "tenant", &next)
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
    f.cleanup().await;
    let applied = gateway
        .get_workflow_execution("ns", "tenant", &parent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(applied.received_signal_ids.len(), 1);
    assert!(applied.buffered_signals.is_empty());
    assert_eq!(
        applied
            .checkpoints
            .iter()
            .filter(|c| c.name == "child-result")
            .count(),
        1
    );
    let deliveries = gateway
        .poll_worker_tasks("ns", "tenant", "work", 10, Some(60), None)
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].task_id, next);
    assert!(
        !gateway
            .get_workflow_execution("ns", "tenant", &child)
            .await
            .unwrap()
            .unwrap()
            .close_pending
    );
}

#[tokio::test]
async fn failed_destination_does_not_block_other_deliveries_or_records() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let bad = WorkerTask::new_at("ns", "tenant", "bad", "job", json!({}), f.clock.now())
        .for_workflow("missing");
    gateway.enqueue_worker_task(bad).await.unwrap();
    let bad = gateway
        .poll_worker_tasks("ns", "tenant", "bad", 1, Some(60), None)
        .await
        .unwrap()
        .remove(0);
    gateway
        .fail_worker_task(
            "ns",
            "tenant",
            &bad.task_id,
            bad.lease_token.as_deref().unwrap(),
            "failed",
            false,
        )
        .await
        .unwrap();
    assert_eq!(f.sink.queue.len(), 1);
    let (id, good) = f.workflow(&gateway).await;
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        gateway
            .complete_worker_task(
                "ns",
                "tenant",
                &good.task_id,
                good.lease_token.as_deref().unwrap(),
                json!({})
            )
            .await
            .is_err()
    );
    assert!(gateway.reconcile_worker_handoffs().await.is_err());
    assert_eq!(
        gateway
            .get_workflow_execution("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Completed
    );
    assert!(
        f.task(&good.task_id)
            .await
            .handoff
            .unwrap()
            .completed_at
            .is_some()
    );
    let pending = f.task(&bad.task_id).await.handoff.unwrap();
    assert!(pending.workflow_pending);
    assert!(!pending.dlq_pending);
    assert_eq!(f.sink.attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exhausted_lease_persists_chain_handoff_and_acknowledges_dlq_independently() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.chain(&gateway).await;
    f.clock.advance_to(Duration::from_secs(60)).unwrap();
    f.fault
        .fail_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(
        gateway
            .poll_worker_tasks("ns", "tenant", "work", 1, Some(60), None)
            .await
            .unwrap()
            .is_empty()
    );
    let pending = f.task(&task.task_id).await;
    assert_eq!(pending.status, acteon_core::WorkerTaskStatus::Failed);
    assert!(pending.handoff.as_ref().unwrap().chain_pending);
    assert!(!pending.handoff.as_ref().unwrap().dlq_pending);
    f.cleanup().await;
    assert_eq!(
        gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        acteon_core::ChainStatus::Failed
    );
    assert_eq!(f.sink.queue.len(), 1);
    assert!(
        f.task(&task.task_id)
            .await
            .handoff
            .unwrap()
            .completed_at
            .is_some()
    );
}

#[tokio::test]
async fn late_workflow_handoff_cannot_overwrite_cancellation_after_lock_expiry() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.workflow(&gateway).await;
    let resume = f
        .fault
        .pause_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let mut completion = Box::pin(gateway.complete_worker_task(
        "ns",
        "tenant",
        &task.task_id,
        task.lease_token.as_deref().unwrap(),
        json!({"ok":1}),
    ));
    assert!(poll!(&mut completion).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.clock.advance_to(Duration::from_secs(31)).unwrap();
    f.gateway()
        .cancel_workflow("ns", "tenant", &id, Some("cancelled concurrently".into()))
        .await
        .unwrap();
    resume.send(()).unwrap();
    completion.await.unwrap();
    assert_eq!(
        gateway
            .get_workflow_execution("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Cancelled
    );
}

#[tokio::test]
async fn late_chain_handoff_cannot_overwrite_cancellation_after_lock_expiry() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (id, task) = f.chain(&gateway).await;
    let resume = f
        .fault
        .pause_next(
            KeyKind::Chain,
            WriteOperation::CompareAndSwap,
            FaultTiming::Before,
        )
        .unwrap();
    let mut completion = Box::pin(gateway.complete_worker_task(
        "ns",
        "tenant",
        &task.task_id,
        task.lease_token.as_deref().unwrap(),
        json!({"ok":1}),
    ));
    assert!(poll!(&mut completion).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.clock.advance_to(Duration::from_secs(60)).unwrap();
    f.gateway()
        .cancel_chain(
            "ns",
            "tenant",
            &id,
            Some("cancelled concurrently".into()),
            None,
        )
        .await
        .unwrap();
    resume.send(()).unwrap();
    completion.await.unwrap();
    assert_eq!(
        gateway
            .get_chain_status("ns", "tenant", &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        acteon_core::ChainStatus::Cancelled
    );
    f.cleanup().await;
    assert!(
        f.task(&task.task_id)
            .await
            .handoff
            .unwrap()
            .completed_at
            .is_some()
    );
}

#[tokio::test]
async fn late_timer_delivery_cannot_acknowledge_new_cancellation_effects() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let (parent, _) = f.workflow(&gateway).await;
    let child = gateway
        .start_child_workflow(
            "ns",
            "tenant",
            &parent,
            "child",
            "flow",
            Some("children"),
            json!({}),
            acteon_core::ParentClosePolicy::Abandon,
        )
        .await
        .unwrap();
    let task = gateway
        .poll_worker_tasks("ns", "tenant", "children", 1, Some(60), None)
        .await
        .unwrap()
        .remove(0);
    let resume = f
        .fault
        .pause_next(
            kind("workflow_timer"),
            WriteOperation::IndexTimeout,
            FaultTiming::Before,
        )
        .unwrap();
    let mut completion = Box::pin(gateway.complete_worker_task(
        "ns",
        "tenant",
        &task.task_id,
        task.lease_token.as_deref().unwrap(),
        json!({"directive":"sleep","checkpoint":"nap","seconds":60}),
    ));
    assert!(poll!(&mut completion).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.clock.advance_to(Duration::from_secs(31)).unwrap();
    f.fault
        .fail_next(
            kind("workflow_exec"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        f.gateway()
            .cancel_workflow("ns", "tenant", &child, None)
            .await
            .is_err()
    );
    resume.send(()).unwrap();
    completion.await.unwrap();
    let cancelled = gateway
        .get_workflow_execution("ns", "tenant", &child)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.status, WorkflowStatus::Cancelled);
    assert!(
        cancelled.close_pending,
        "timer delivery did not notify the parent of cancellation"
    );
    f.cleanup().await;
    let parent = gateway
        .get_workflow_execution("ns", "tenant", &parent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.received_signal_ids.len(), 1);
    assert_eq!(parent.buffered_signals.len(), 1);
    assert_eq!(
        parent.buffered_signals[0].payload,
        json!({"status":"cancelled"})
    );
    assert!(
        !gateway
            .get_workflow_execution("ns", "tenant", &child)
            .await
            .unwrap()
            .unwrap()
            .close_pending
    );
}
