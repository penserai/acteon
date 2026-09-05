use std::sync::Arc;
use std::time::Duration;

use acteon_core::{WorkerTask, WorkerTaskStatus};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_time::{Clock, ManualClock};
use serde_json::json;

fn fixture() -> (Arc<ManualClock>, Arc<MemoryStateStore>, Gateway) {
    let clock = Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    ));
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let gateway = GatewayBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
        .build()
        .unwrap();
    (clock, store, gateway)
}

#[tokio::test]
async fn expired_queue_owner_cannot_heartbeat_complete_or_fail_before_reaping() {
    for operation in ["heartbeat", "complete", "fail"] {
        let (clock, store, gateway) = fixture();
        let mut task = WorkerTask::new("ns", "tenant", "queue", "work", json!({}));
        task.status = WorkerTaskStatus::Leased;
        task.attempt = 1;
        task.lease_token = Some("expired-owner".into());
        task.lease_expires_at = Some(clock.now());
        let key = StateKey::new(
            "ns",
            "tenant",
            KeyKind::Custom("worker_task".into()),
            &task.task_id,
        );
        let original = serde_json::to_string(&task).unwrap();
        store.set(&key, &original, None).await.unwrap();
        let result = match operation {
            "heartbeat" => {
                gateway
                    .heartbeat_worker_task("ns", "tenant", &task.task_id, "expired-owner", Some(1))
                    .await
            }
            "complete" => {
                gateway
                    .complete_worker_task(
                        "ns",
                        "tenant",
                        &task.task_id,
                        "expired-owner",
                        json!({"ok":true}),
                    )
                    .await
            }
            _ => {
                gateway
                    .fail_worker_task(
                        "ns",
                        "tenant",
                        &task.task_id,
                        "expired-owner",
                        "failed",
                        true,
                    )
                    .await
            }
        };
        assert!(
            result.is_err(),
            "expired {operation} must fail without requiring a reaper poll"
        );
        assert_eq!(store.get(&key).await.unwrap(), Some(original));
    }
}

#[tokio::test]
async fn worker_queue_leases_and_retry_backoff_share_virtual_time() {
    let (clock, _store, gateway) = fixture();
    let task = WorkerTask::new("ns", "tenant", "queue", "work", json!({}));
    gateway.enqueue_worker_task(task).await.unwrap();
    let leased = gateway
        .poll_worker_tasks("ns", "tenant", "queue", 1, Some(1), Some("first"))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(leased.updated_at, clock.now());
    assert_eq!(
        leased.lease_expires_at,
        Some(clock.now() + chrono::Duration::seconds(1))
    );
    clock.advance_to(Duration::from_millis(999)).unwrap();
    assert!(
        gateway
            .poll_worker_tasks("ns", "tenant", "queue", 1, Some(1), Some("second"))
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_to(Duration::from_millis(1_000)).unwrap();
    assert!(
        gateway
            .poll_worker_tasks("ns", "tenant", "queue", 1, Some(1), Some("second"))
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_to(Duration::from_millis(2_999)).unwrap();
    assert!(
        gateway
            .poll_worker_tasks("ns", "tenant", "queue", 1, Some(1), Some("second"))
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_to(Duration::from_millis(3_000)).unwrap();
    let successor = gateway
        .poll_worker_tasks("ns", "tenant", "queue", 1, Some(1), Some("second"))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(successor.attempt, 2);
    assert_ne!(successor.lease_token, leased.lease_token);
    assert!(
        gateway
            .complete_worker_task(
                "ns",
                "tenant",
                &leased.task_id,
                leased.lease_token.as_deref().unwrap(),
                json!({})
            )
            .await
            .is_err()
    );
    gateway
        .complete_worker_task(
            "ns",
            "tenant",
            &successor.task_id,
            successor.lease_token.as_deref().unwrap(),
            json!({}),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn interrupted_scheduled_handoff_remains_discoverable_after_restart() {
    use acteon_gateway::{
        BackgroundConfig, BackgroundJob, BackgroundProcessorBuilder, GatewayMetrics, GroupManager,
    };
    use tokio::sync::mpsc;
    let (clock, store, _gateway) = fixture();
    let action = acteon_core::Action::new("ns", "tenant", "effect", "scheduled", json!({}));
    let record = json!({"action_id":"schedule","action":action,"scheduled_for":clock.now(),"created_at":clock.now()});
    let payload = StateKey::new("ns", "tenant", KeyKind::ScheduledAction, "schedule");
    let pending = StateKey::new("ns", "tenant", KeyKind::PendingScheduled, "schedule");
    // The preceding format carried a TTL. Claiming it must remove that expiry
    // so restart recovery remains possible after the old retention window.
    store
        .set(&payload, &record.to_string(), Some(Duration::from_secs(1)))
        .await
        .unwrap();
    store
        .set(&pending, &clock.now().timestamp_millis().to_string(), None)
        .await
        .unwrap();
    store
        .index_timeout(&pending, clock.now().timestamp_millis())
        .await
        .unwrap();
    for pass in 0..2 {
        let (tx, mut rx) = mpsc::channel(1);
        let (mut worker, _shutdown) = BackgroundProcessorBuilder::new()
            .clock(clock.clone())
            .state(store.clone())
            .group_manager(Arc::new(GroupManager::with_clock(clock.clone())))
            .metrics(Arc::new(GatewayMetrics::default()))
            .config(BackgroundConfig {
                enable_scheduled_actions: true,
                ..Default::default()
            })
            .scheduled_action_channel(tx)
            .build()
            .unwrap();
        worker.tick(BackgroundJob::ScheduledActions).await.unwrap();
        assert_eq!(rx.try_recv().unwrap().action_id, "schedule");
        assert!(
            store.get(&pending).await.unwrap().is_some(),
            "handoff {pass} must retain discovery until acknowledgement"
        );
        assert!(
            store
                .get_expired_timeouts(clock.now().timestamp_millis())
                .await
                .unwrap()
                .contains(&pending.canonical())
        );
        // Drop both worker and unacknowledged delivery, then restart after expiry.
        clock.advance_to(Duration::from_secs(60)).unwrap();
    }
}

struct Effect {
    clock: Arc<ManualClock>,
    delay: Duration,
    calls: std::sync::atomic::AtomicUsize,
    actions: std::sync::Mutex<Vec<acteon_core::Action>>,
}

#[async_trait::async_trait]
impl acteon_provider::DynProvider for Effect {
    fn name(&self) -> &'static str {
        "effect"
    }
    async fn execute(
        &self,
        action: &acteon_core::Action,
    ) -> Result<acteon_core::ProviderResponse, acteon_provider::ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.clock.sleep(self.delay).await;
        self.actions.lock().unwrap().push(action.clone());
        Ok(acteon_core::ProviderResponse::success(json!({"ok":true})))
    }
    async fn health_check(&self) -> Result<(), acteon_provider::ProviderError> {
        Ok(())
    }
}

struct ScheduledFixture {
    clock: Arc<ManualClock>,
    store: Arc<MemoryStateStore>,
    gateway: Gateway,
    worker: acteon_gateway::BackgroundProcessor,
    rx: tokio::sync::mpsc::Receiver<acteon_gateway::background::ScheduledActionDueEvent>,
    effect: Arc<Effect>,
    id: String,
}

impl ScheduledFixture {
    async fn due(&mut self) -> acteon_gateway::background::ScheduledActionDueEvent {
        self.clock.advance_to(Duration::from_secs(1)).unwrap();
        self.worker
            .tick(acteon_gateway::BackgroundJob::ScheduledActions)
            .await
            .unwrap();
        self.rx.try_recv().unwrap()
    }
    fn pending(&self) -> StateKey {
        StateKey::new("ns", "tenant", KeyKind::PendingScheduled, &self.id)
    }
    fn record(&self) -> StateKey {
        StateKey::new("ns", "tenant", KeyKind::ScheduledAction, &self.id)
    }
}

async fn scheduled_fixture(payload: serde_json::Value, delay: Duration) -> ScheduledFixture {
    use acteon_rules::RuleFrontend;
    let (clock, store, _) = fixture();
    let effect = Arc::new(Effect {
        clock: clock.clone(),
        delay,
        calls: std::sync::atomic::AtomicUsize::new(0),
        actions: std::sync::Mutex::new(Vec::new()),
    });
    let rules = acteon_rules_yaml::YamlFrontend.parse("rules:\n  - name: later\n    condition: {field: action.action_type, eq: scheduled}\n    action: {type: schedule, delay_seconds: 1}\n").unwrap();
    let encryptor = Arc::new(acteon_crypto::PayloadEncryptor::new(
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap(),
    ));
    let gateway = GatewayBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
        .payload_encryptor(encryptor.clone())
        .provider(effect.clone())
        .rules(rules)
        .executor_config(acteon_executor::ExecutorConfig {
            execution_timeout: Duration::from_secs(180),
            max_retries: 0,
            ..Default::default()
        })
        .build()
        .unwrap();
    let action = acteon_core::Action::new("ns", "tenant", "effect", "scheduled", payload);
    let acteon_core::ActionOutcome::Scheduled { action_id: id, .. } =
        gateway.dispatch(action, None).await.unwrap()
    else {
        panic!("expected scheduled outcome")
    };
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let (worker, _shutdown) = acteon_gateway::BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .group_manager(gateway.group_manager())
        .metrics(gateway.metrics_arc())
        .config(acteon_gateway::BackgroundConfig {
            enable_scheduled_actions: true,
            ..Default::default()
        })
        .scheduled_action_channel(tx)
        .payload_encryptor(encryptor)
        .build()
        .unwrap();
    ScheduledFixture {
        clock,
        store,
        gateway,
        worker,
        rx,
        effect,
        id,
    }
}

#[tokio::test]
async fn scheduled_consumer_executes_original_payload_once_without_rescheduling() {
    let payload = json!(["array payload",{"value":7}]);
    let mut f = scheduled_fixture(payload.clone(), Duration::ZERO).await;
    let mut receipt = f.due().await;
    receipt.action.payload = json!({"tampered":"event payload must not be authoritative"});
    let result = f
        .gateway
        .dispatch_scheduled_action(&receipt)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, acteon_core::ActionOutcome::Executed(_)));
    assert!(
        f.gateway
            .dispatch_scheduled_action(&receipt)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
    assert_eq!(f.effect.actions.lock().unwrap()[0].payload, payload);
    assert!(f.store.get(&f.pending()).await.unwrap().is_none());
    let raw = f.store.get(&f.record()).await.unwrap().unwrap();
    assert!(acteon_crypto::is_encrypted(&raw));
    let record: serde_json::Value = serde_json::from_str(
        &f.gateway
            .payload_encryptor()
            .unwrap()
            .decrypt_str(&raw)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["completed_at"], json!(f.clock.now()));
    assert!(!record["outcome"].is_null());
    assert_eq!(
        f.store
            .scan_keys_by_kind(KeyKind::ScheduledAction)
            .await
            .unwrap()
            .len(),
        1,
        "no recursive schedule or separate ephemeral claim"
    );
}

#[tokio::test]
async fn scheduled_recovery_rejects_old_and_duplicate_receipts() {
    use acteon_gateway::BackgroundJob;
    let mut f = scheduled_fixture(json!({}), Duration::ZERO).await;
    let old = f.due().await;
    f.clock.advance_to(Duration::from_secs(61)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    let current = f.rx.try_recv().unwrap();
    assert_ne!(old.delivery_token, current.delivery_token);
    assert!(
        f.gateway
            .dispatch_scheduled_action(&old)
            .await
            .unwrap()
            .is_none()
    );
    let (first, second) = tokio::join!(
        f.gateway.dispatch_scheduled_action(&current),
        f.gateway.dispatch_scheduled_action(&current)
    );
    assert_eq!(
        usize::from(first.unwrap().is_some()) + usize::from(second.unwrap().is_some()),
        1
    );
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cleanup_recovers_missing_schedule_indexes_and_removes_completed_discovery() {
    use acteon_gateway::BackgroundJob;
    let mut f = scheduled_fixture(json!({}), Duration::ZERO).await;
    f.store.delete(&f.pending()).await.unwrap();
    f.store.remove_timeout_index(&f.pending()).await.unwrap();
    f.clock.advance_to(Duration::from_secs(2)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    assert!(f.rx.try_recv().is_err());
    f.worker.tick(BackgroundJob::Cleanup).await.unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    let event = f.rx.try_recv().unwrap();
    f.gateway
        .dispatch_scheduled_action(&event)
        .await
        .unwrap()
        .unwrap();
    // Crash after completion CAS but before discovery cleanup: a stale index
    // must be cleaned without another provider effect.
    f.store.set(&f.pending(), "0", None).await.unwrap();
    f.store.index_timeout(&f.pending(), 0).await.unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    assert!(f.rx.try_recv().is_err());
    assert!(f.store.get(&f.pending()).await.unwrap().is_none());
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn long_scheduled_dispatch_renews_its_lease_using_virtual_time() {
    use acteon_gateway::BackgroundJob;
    use futures::poll;
    let mut f = scheduled_fixture(json!({}), Duration::from_secs(80)).await;
    let receipt = f.due().await;
    let mut dispatch = Box::pin(f.gateway.dispatch_scheduled_action(&receipt));
    assert!(poll!(&mut dispatch).is_pending());
    for seconds in [21, 41, 61] {
        f.clock.advance_to(Duration::from_secs(seconds)).unwrap();
        assert!(poll!(&mut dispatch).is_pending());
        f.worker
            .tick(BackgroundJob::ScheduledActions)
            .await
            .unwrap();
        assert!(
            f.rx.try_recv().is_err(),
            "renewed lease prevents a second owner"
        );
    }
    f.clock.advance_to(Duration::from_secs(81)).unwrap();
    assert!(matches!(
        poll!(&mut dispatch),
        std::task::Poll::Ready(Ok(Some(_)))
    ));
    drop(dispatch);
    assert_eq!(f.clock.pending_timers(), 0);
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_scheduled_consumer_is_reclaimed_after_its_last_renewal() {
    use acteon_gateway::BackgroundJob;
    use futures::poll;
    let mut f = scheduled_fixture(json!({}), Duration::from_secs(80)).await;
    let receipt = f.due().await;
    let mut dispatch = Box::pin(f.gateway.dispatch_scheduled_action(&receipt));
    assert!(poll!(&mut dispatch).is_pending());
    f.clock.advance_to(Duration::from_secs(21)).unwrap();
    assert!(poll!(&mut dispatch).is_pending());
    drop(dispatch);
    assert_eq!(f.clock.pending_timers(), 0);
    f.clock.advance_to(Duration::from_millis(80_999)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    assert!(f.rx.try_recv().is_err());
    f.clock.advance_to(Duration::from_secs(81)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    assert_ne!(
        f.rx.try_recv().unwrap().delivery_token,
        receipt.delivery_token
    );
    assert!(f.effect.actions.lock().unwrap().is_empty());
}

// Keep the ordered restart and deadline observations together.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn workflow_restart_preserves_checkpoints_and_fires_sleep_at_exact_deadline() {
    use acteon_core::{WorkflowDirective, WorkflowStatus};
    use acteon_gateway::{BackgroundConfig, BackgroundJob, BackgroundProcessorBuilder};
    let (clock, store, gateway) = fixture();
    let execution = gateway
        .start_workflow(
            "ns",
            "tenant",
            "deploy",
            "deployments",
            json!({"release":7}),
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(execution.created_at, clock.now());
    let first = gateway
        .poll_worker_tasks("ns", "tenant", "deployments", 1, Some(1), Some("old"))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(first.created_at, clock.now());
    clock.advance_to(Duration::from_millis(500)).unwrap();
    let checkpoint = gateway
        .record_workflow_checkpoint(
            "ns",
            "tenant",
            &execution.execution_id,
            "build",
            json!({"artifact":"release-7"}),
        )
        .await
        .unwrap();
    assert_eq!(checkpoint.recorded_at, clock.now());
    drop(gateway);
    let recovered = GatewayBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
        .build()
        .unwrap();
    clock.advance_to(Duration::from_secs(1)).unwrap();
    assert!(
        recovered
            .poll_worker_tasks("ns", "tenant", "deployments", 1, Some(1), Some("new"))
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_to(Duration::from_secs(3)).unwrap();
    let next = recovered
        .poll_worker_tasks("ns", "tenant", "deployments", 1, Some(1), Some("new"))
        .await
        .unwrap()
        .remove(0);
    assert!(
        recovered
            .complete_worker_task(
                "ns",
                "tenant",
                &first.task_id,
                first.lease_token.as_deref().unwrap(),
                json!({})
            )
            .await
            .is_err()
    );
    let replayed = recovered
        .record_workflow_checkpoint(
            "ns",
            "tenant",
            &execution.execution_id,
            "build",
            json!({"artifact":"wrong"}),
        )
        .await
        .unwrap();
    assert_eq!(replayed.data, checkpoint.data);
    assert_eq!(replayed.recorded_at, checkpoint.recorded_at);
    recovered
        .complete_worker_task(
            "ns",
            "tenant",
            &next.task_id,
            next.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::Sleep {
                checkpoint: "rollout".into(),
                seconds: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let gw = Arc::new(tokio::sync::RwLock::new(recovered));
    let (mut worker, _shutdown) = BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store)
        .group_manager(gw.read().await.group_manager())
        .metrics(gw.read().await.metrics_arc())
        .gateway(gw.clone())
        .config(BackgroundConfig {
            enable_template_sync: false,
            ..Default::default()
        })
        .build()
        .unwrap();
    clock.advance_to(Duration::from_millis(3_999)).unwrap();
    worker.tick(BackgroundJob::ChainAdvance).await.unwrap();
    assert_eq!(
        gw.read()
            .await
            .get_workflow_execution("ns", "tenant", &execution.execution_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::WaitingTimer
    );
    clock.advance_to(Duration::from_secs(4)).unwrap();
    worker.tick(BackgroundJob::ChainAdvance).await.unwrap();
    let gw = gw.read().await;
    let resumed = gw
        .get_workflow_execution("ns", "tenant", &execution.execution_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.status, WorkflowStatus::Running);
    assert_eq!(
        resumed.checkpoint("rollout").unwrap().recorded_at,
        clock.now()
    );
    let final_task = gw
        .poll_worker_tasks("ns", "tenant", "deployments", 1, Some(1), Some("new"))
        .await
        .unwrap()
        .remove(0);
    gw.complete_worker_task(
        "ns",
        "tenant",
        &final_task.task_id,
        final_task.lease_token.as_deref().unwrap(),
        serde_json::to_value(WorkflowDirective::Complete {
            result: json!({"deployed":7}),
        })
        .unwrap(),
    )
    .await
    .unwrap();
    let history = gw
        .get_execution_history("ns", "tenant", &execution.execution_id)
        .await
        .unwrap();
    assert_eq!(
        history.events.first().unwrap().timestamp,
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    );
    assert_eq!(history.events.last().unwrap().timestamp, clock.now());
    assert_eq!(
        gw.get_workflow_execution("ns", "tenant", &execution.execution_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Completed
    );
}

#[tokio::test]
async fn late_acknowledgement_cannot_overwrite_a_successor_lease() {
    use acteon_gateway::BackgroundJob;
    use futures::poll;
    let mut f = scheduled_fixture(json!({}), Duration::from_secs(80)).await;
    let receipt = f.due().await;
    let mut dispatch = Box::pin(f.gateway.dispatch_scheduled_action(&receipt));
    assert!(poll!(&mut dispatch).is_pending());
    // Model a consumer that cannot poll its renewal future while another
    // processor takes ownership. Its external effect may still complete.
    f.clock.advance_to(Duration::from_secs(61)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    let successor = f.rx.try_recv().unwrap();
    f.clock.advance_to(Duration::from_secs(81)).unwrap();
    assert!(matches!(
        poll!(&mut dispatch),
        std::task::Poll::Ready(Err(_))
    ));
    drop(dispatch);
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
    let raw = f.store.get(&f.record()).await.unwrap().unwrap();
    assert!(acteon_crypto::is_encrypted(&raw));
    let record: serde_json::Value = serde_json::from_str(
        &f.gateway
            .payload_encryptor()
            .unwrap()
            .decrypt_str(&raw)
            .unwrap(),
    )
    .unwrap();
    assert!(record["completed_at"].is_null());
    assert_eq!(record["delivery"]["token"], successor.delivery_token);
    assert!(f.store.get(&f.pending()).await.unwrap().is_some());
}

#[tokio::test]
async fn active_schedules_survive_long_outages_and_completed_records_expire() {
    use acteon_gateway::BackgroundJob;
    let mut f = scheduled_fixture(json!({}), Duration::ZERO).await;
    let old = f.due().await;
    let outage = 8 * 86_400;
    f.clock.advance_to(Duration::from_secs(outage)).unwrap();
    f.worker
        .tick(BackgroundJob::ScheduledActions)
        .await
        .unwrap();
    let next = f.rx.try_recv().unwrap();
    assert_ne!(next.delivery_token, old.delivery_token);
    assert!(
        f.gateway
            .dispatch_scheduled_action(&next)
            .await
            .unwrap()
            .is_some()
    );
    f.clock
        .advance_to(Duration::from_secs(outage + 86_400 - 1))
        .unwrap();
    assert!(f.store.get(&f.record()).await.unwrap().is_some());
    f.clock
        .advance_to(Duration::from_secs(outage + 86_400))
        .unwrap();
    assert!(f.store.get(&f.record()).await.unwrap().is_none());
    assert!(
        f.gateway
            .dispatch_scheduled_action(&next)
            .await
            .unwrap()
            .is_none()
    );
    assert!(f.store.get(&f.pending()).await.unwrap().is_none());
    assert_eq!(f.effect.actions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn corrupt_schedules_are_retained_without_blocking_other_deliveries() {
    use acteon_gateway::BackgroundJob;
    let mut f = scheduled_fixture(json!({}), Duration::ZERO).await;
    let original = f.store.get(&f.record()).await.unwrap().unwrap();
    let foreign_key = acteon_crypto::PayloadEncryptor::new(
        acteon_crypto::parse_master_key(&"24".repeat(32)).unwrap(),
    );
    let unreadable = foreign_key.encrypt_str("unreadable record").unwrap();
    let bad_rows = [
        ("wrong-scope", original),
        ("unreadable", unreadable),
        ("invalid", "{}".into()),
    ];
    for (id, raw) in &bad_rows {
        let key = StateKey::new("ns", "tenant", KeyKind::ScheduledAction, *id);
        let pending = StateKey::new("ns", "tenant", KeyKind::PendingScheduled, *id);
        f.store.set(&key, raw, None).await.unwrap();
        f.store.set(&pending, "0", None).await.unwrap();
        f.store.index_timeout(&pending, 0).await.unwrap();
    }
    f.worker.tick(BackgroundJob::Cleanup).await.unwrap();
    let receipt = f.due().await;
    assert_eq!(receipt.action_id, f.id);
    assert!(f.rx.try_recv().is_err());
    assert!(
        f.gateway
            .dispatch_scheduled_action(&receipt)
            .await
            .unwrap()
            .is_some()
    );
    for (id, raw) in bad_rows {
        let key = StateKey::new("ns", "tenant", KeyKind::ScheduledAction, id);
        assert_eq!(f.store.get(&key).await.unwrap(), Some(raw));
    }
}
