use std::sync::Arc;
use std::time::Duration;

use acteon_core::{WorkerTask, WorkerTaskStatus};
use acteon_gateway::{BackgroundJob, BackgroundProcessorBuilder, Gateway, GatewayBuilder};
use acteon_state::{
    KeyKind, StateKey, StateStore,
    testing::faults::{FaultStore, FaultTiming, WriteOperation},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use futures::poll;
use serde_json::json;

fn kind(name: &str) -> KeyKind {
    KeyKind::Custom(name.into())
}
fn row(id: &str) -> StateKey {
    StateKey::new("ns", "tenant", kind("worker_task"), id)
}
fn index(queue: &str, id: &str) -> StateKey {
    StateKey::new(
        "ns",
        "tenant",
        kind("queue_pending"),
        format!("{queue}:{id}"),
    )
}
struct Fixture {
    clock: Arc<ManualClock>,
    store: Arc<MemoryStateStore>,
    fault: Arc<FaultStore>,
}
impl Fixture {
    fn new() -> Self {
        let clock = Arc::new(ManualClock::new(
            chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        ));
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let fault = Arc::new(FaultStore::new(store.clone()));
        Self {
            clock,
            store,
            fault,
        }
    }
    fn gateway(&self) -> Gateway {
        GatewayBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .lock(Arc::new(MemoryDistributedLock::with_clock(
                self.clock.clone(),
            )))
            .build()
            .unwrap()
    }
    fn task(&self) -> WorkerTask {
        WorkerTask::new_at(
            "ns",
            "tenant",
            "q",
            "work",
            json!({"n":1}),
            self.clock.now(),
        )
    }
    fn advance(&self, ms: u64) {
        self.clock.advance_to(Duration::from_millis(ms)).unwrap();
    }
    async fn load(&self, id: &str) -> WorkerTask {
        serde_json::from_str(&self.store.get(&row(id)).await.unwrap().unwrap()).unwrap()
    }
    async fn cleanup(&self, gateway: Gateway) {
        let gateway = Arc::new(tokio::sync::RwLock::new(gateway));
        let (mut worker, _shutdown) = BackgroundProcessorBuilder::new()
            .clock(self.clock.clone())
            .state(self.fault.clone())
            .group_manager(gateway.read().await.group_manager())
            .metrics(gateway.read().await.metrics_arc())
            .gateway(gateway.clone())
            .build()
            .unwrap();
        worker.tick(BackgroundJob::Cleanup).await.unwrap();
    }
}
async fn lease(gateway: &Gateway) -> Vec<WorkerTask> {
    gateway
        .poll_worker_tasks("ns", "tenant", "q", 1, Some(1), Some("worker"))
        .await
        .unwrap()
}

#[tokio::test]
async fn enqueue_cannot_reset_an_existing_lease() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let task = f.task();
    gateway.enqueue_worker_task(task.clone()).await.unwrap();
    let leased = lease(&gateway).await.remove(0);
    assert!(gateway.enqueue_worker_task(task).await.is_err());
    assert_eq!(
        f.load(&leased.task_id).await.lease_token,
        leased.lease_token
    );
}

#[tokio::test]
async fn cleanup_repairs_interrupted_initial_discovery() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let task = f.task();
    f.fault
        .fail_next(
            kind("queue_pending"),
            WriteOperation::Set,
            FaultTiming::Before,
        )
        .unwrap();
    assert!(gateway.enqueue_worker_task(task.clone()).await.is_err());
    assert_eq!(f.fault.consumed(), 1);
    assert!(lease(&gateway).await.is_empty());
    drop(gateway);
    f.cleanup(f.gateway()).await;
    let recovered = lease(&f.gateway()).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].task_id, task.task_id);
}

#[tokio::test]
async fn lost_create_acknowledgement_preserves_one_recoverable_task() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let task = f.task();
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CheckAndSet,
            FaultTiming::After,
        )
        .unwrap();
    assert!(gateway.enqueue_worker_task(task.clone()).await.is_err());
    assert_eq!(f.fault.consumed(), 1);
    assert_eq!(f.load(&task.task_id).await.attempt, 0);
    assert!(gateway.enqueue_worker_task(task.clone()).await.is_err());
    assert!(lease(&gateway).await.is_empty());
    f.cleanup(f.gateway()).await;
    let recovered = lease(&gateway).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].task_id, task.task_id);
    assert_eq!(recovered[0].attempt, 1);
}

#[tokio::test]
async fn lost_retry_acknowledgement_does_not_strand_the_task() {
    let f = Fixture::new();
    let gateway = f.gateway();
    gateway.enqueue_worker_task(f.task()).await.unwrap();
    let task = lease(&gateway).await.remove(0);
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        gateway
            .fail_worker_task(
                "ns",
                "tenant",
                &task.task_id,
                task.lease_token.as_deref().unwrap(),
                "retry",
                true
            )
            .await
            .is_err()
    );
    assert_eq!(f.fault.consumed(), 1);
    assert_eq!(
        f.load(&task.task_id).await.status,
        WorkerTaskStatus::Pending
    );
    drop(gateway);
    f.advance(2_000);
    let recovered = lease(&f.gateway()).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].attempt, 2);
}

#[tokio::test]
async fn delayed_old_poll_cannot_erase_requeued_discovery() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let task = f.task();
    gateway.enqueue_worker_task(task.clone()).await.unwrap();
    let resume = f
        .fault
        .pause_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    let mut old = Box::pin(lease(&gateway));
    assert!(poll!(&mut old).is_pending());
    assert_eq!(f.fault.consumed(), 1);
    f.advance(1_000);
    assert!(lease(&f.gateway()).await.is_empty());
    assert_eq!(
        f.load(&task.task_id).await.status,
        WorkerTaskStatus::Pending
    );
    f.advance(2_000);
    resume.send(()).unwrap();
    assert_eq!(old.await.len(), 1);
    f.advance(3_000);
    let recovered = lease(&f.gateway()).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].attempt, 2);
}

#[tokio::test]
async fn foreign_queue_index_cannot_lease_another_queues_task() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let mut task = f.task();
    task.queue = "private".into();
    gateway.enqueue_worker_task(task.clone()).await.unwrap();
    f.store
        .set(&index("q", &task.task_id), "pending", None)
        .await
        .unwrap();
    assert!(lease(&gateway).await.is_empty());
    assert_eq!(
        f.load(&task.task_id).await.status,
        WorkerTaskStatus::Pending
    );
}

#[tokio::test]
async fn ambiguous_reclaim_preserves_discovery_and_retry_budget() {
    let f = Fixture::new();
    let gateway = f.gateway();
    gateway.enqueue_worker_task(f.task()).await.unwrap();
    let task = lease(&gateway).await.remove(0);
    f.advance(1_000);
    f.fault
        .fail_next(
            kind("worker_task"),
            WriteOperation::CompareAndSwap,
            FaultTiming::After,
        )
        .unwrap();
    assert!(
        gateway
            .poll_worker_tasks("ns", "tenant", "q", 1, Some(1), None)
            .await
            .is_err()
    );
    assert_eq!(f.fault.consumed(), 1);
    f.advance(2_999);
    assert!(lease(&f.gateway()).await.is_empty());
    f.advance(3_000);
    let next = lease(&f.gateway()).await.remove(0);
    assert_eq!(next.task_id, task.task_id);
    assert_eq!(next.attempt, 2);
    assert_ne!(next.lease_token, task.lease_token);
}

#[tokio::test]
async fn lease_recovery_uses_record_deadline_instead_of_legacy_hint() {
    let f = Fixture::new();
    let gateway = f.gateway();
    gateway.enqueue_worker_task(f.task()).await.unwrap();
    let task = lease(&gateway).await.remove(0);
    let legacy = StateKey::new(
        "ns",
        "tenant",
        kind("queue_leased"),
        format!("q:{}", task.task_id),
    );
    f.store
        .set(
            &legacy,
            &(f.clock.now().timestamp_millis() + 999_999).to_string(),
            None,
        )
        .await
        .unwrap();
    // An old-version leased task may be missing the active discovery record.
    f.store.delete(&index("q", &task.task_id)).await.unwrap();
    f.cleanup(f.gateway()).await;
    f.advance(1_000);
    assert!(lease(&gateway).await.is_empty());
    f.advance(3_000);
    assert_eq!(lease(&gateway).await.remove(0).attempt, 2);
}

#[tokio::test]
async fn terminal_cleanup_failure_never_releases_another_task_attempt() {
    for operation in ["complete", "fail", "cancel", "reap"] {
        let f = Fixture::new();
        let gateway = f.gateway();
        gateway
            .enqueue_worker_task(f.task().with_max_attempts(1))
            .await
            .unwrap();
        let task = lease(&gateway).await.remove(0);
        f.fault
            .fail_next(
                kind("queue_pending"),
                WriteOperation::Delete,
                FaultTiming::Before,
            )
            .unwrap();
        match operation {
            "complete" => {
                gateway
                    .complete_worker_task(
                        "ns",
                        "tenant",
                        &task.task_id,
                        task.lease_token.as_deref().unwrap(),
                        json!({"ok":true}),
                    )
                    .await
                    .unwrap();
            }
            "fail" => {
                gateway
                    .fail_worker_task(
                        "ns",
                        "tenant",
                        &task.task_id,
                        task.lease_token.as_deref().unwrap(),
                        "failed",
                        true,
                    )
                    .await
                    .unwrap();
            }
            "cancel" => {
                gateway
                    .cancel_worker_task("ns", "tenant", &task.task_id)
                    .await
                    .unwrap();
            }
            _ => {
                f.advance(1_000);
                assert!(lease(&gateway).await.is_empty());
            }
        }
        assert_eq!(f.fault.consumed(), 1);
        assert!(!f.load(&task.task_id).await.status.is_active());
        assert!(
            f.store
                .get(&index("q", &task.task_id))
                .await
                .unwrap()
                .is_some()
        );
        f.cleanup(f.gateway()).await;
        assert!(
            f.store
                .get(&index("q", &task.task_id))
                .await
                .unwrap()
                .is_none()
        );
        assert!(lease(&gateway).await.is_empty());
    }
}

#[tokio::test]
async fn encrypted_queue_records_survive_lease_retry_and_completion() {
    let f = Fixture::new();
    let encryptor = Arc::new(acteon_crypto::PayloadEncryptor::new(
        acteon_crypto::parse_master_key(&"43".repeat(32)).unwrap(),
    ));
    let gateway = GatewayBuilder::new()
        .clock(f.clock.clone())
        .state(f.fault.clone())
        .lock(Arc::new(MemoryDistributedLock::with_clock(f.clock.clone())))
        .payload_encryptor(encryptor)
        .build()
        .unwrap();
    let task = f.task();
    gateway.enqueue_worker_task(task.clone()).await.unwrap();
    assert!(acteon_crypto::is_encrypted(
        &f.store.get(&row(&task.task_id)).await.unwrap().unwrap()
    ));
    let first = lease(&gateway).await.remove(0);
    gateway
        .heartbeat_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            first.lease_token.as_deref().unwrap(),
            Some(2),
        )
        .await
        .unwrap();
    gateway
        .fail_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            first.lease_token.as_deref().unwrap(),
            "retry",
            true,
        )
        .await
        .unwrap();
    assert!(acteon_crypto::is_encrypted(
        &f.store.get(&row(&task.task_id)).await.unwrap().unwrap()
    ));
    f.advance(2_000);
    let next = lease(&gateway).await.remove(0);
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &task.task_id,
            next.lease_token.as_deref().unwrap(),
            json!({"secret":"result"}),
        )
        .await
        .unwrap();
    assert!(acteon_crypto::is_encrypted(
        &f.store.get(&row(&task.task_id)).await.unwrap().unwrap()
    ));
    let listed = gateway
        .list_worker_tasks("ns", "tenant", Some("q"), Some(WorkerTaskStatus::Completed))
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].result, Some(json!({"secret":"result"})));
}

#[tokio::test]
async fn reconciliation_retains_corrupt_rows_and_removes_orphan_hints() {
    let f = Fixture::new();
    let gateway = f.gateway();
    let task = f.task();
    gateway.enqueue_worker_task(task.clone()).await.unwrap();
    f.store.delete(&index("q", &task.task_id)).await.unwrap();
    for (id, raw) in [
        ("bad-json", "{".into()),
        ("wrong-scope", serde_json::to_string(&task).unwrap()),
    ] {
        f.store.set(&row(id), &raw, None).await.unwrap();
        f.store.set(&index("q", id), "active", None).await.unwrap();
    }
    let orphan = index("q", "missing");
    f.store.set(&orphan, "active", None).await.unwrap();
    assert_eq!(gateway.reconcile_worker_task_indexes().await.unwrap(), 1);
    assert!(f.store.get(&orphan).await.unwrap().is_none());
    assert!(f.store.get(&row("bad-json")).await.unwrap().is_some());
    assert!(f.store.get(&row("wrong-scope")).await.unwrap().is_some());
    let recovered = lease(&gateway).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].task_id, task.task_id);
}

#[tokio::test]
async fn concurrent_enqueue_and_poll_never_reset_or_double_lease() {
    let f = Fixture::new();
    let first = f.gateway();
    let second = f.gateway();
    let task = f.task();
    let (a, b) = tokio::join!(
        first.enqueue_worker_task(task.clone()),
        second.enqueue_worker_task(task.clone())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let (a, b) = tokio::join!(lease(&first), lease(&second));
    assert_eq!(a.len() + b.len(), 1);
    assert_eq!(f.load(&task.task_id).await.attempt, 1);
}

#[tokio::test]
async fn enqueue_rejects_malformed_identity_and_noninitial_state() {
    for invalid in [
        "namespace",
        "tenant",
        "id",
        "status",
        "attempt",
        "token",
        "budget",
    ] {
        let f = Fixture::new();
        let mut task = f.task();
        match invalid {
            "namespace" => task.namespace = "ns:extra".into(),
            "tenant" => task.tenant = "tenant:extra".into(),
            "id" => task.task_id = "collision:h".into(),
            "status" => task.status = WorkerTaskStatus::Completed,
            "attempt" => task.attempt = 1,
            "token" => task.lease_token = Some("forged".into()),
            _ => task.max_attempts = 0,
        }
        assert!(
            f.gateway().enqueue_worker_task(task).await.is_err(),
            "accepted {invalid}"
        );
        assert!(
            f.store
                .scan_keys_by_kind(kind("worker_task"))
                .await
                .unwrap()
                .is_empty()
        );
    }
}
