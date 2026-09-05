use std::sync::Arc;
use std::time::Duration;

use acteon_core::{
    Artifact, PauseKind, Task, TaskArtifactUpdateEvent, TaskMessage, TaskPart, TaskRole, TaskState,
};
use acteon_gateway::task_engine::{TaskEngine, TaskScope};
use acteon_gateway::{
    BackgroundConfig, BackgroundJob, BackgroundProcessorBuilder, GatewayMetrics, GroupManager,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use futures::poll;
use serde_json::json;
use tokio::sync::mpsc;

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
    ))
}

fn builder(clock: &Arc<ManualClock>, store: &Arc<MemoryStateStore>) -> BackgroundProcessorBuilder {
    BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .group_manager(Arc::new(GroupManager::with_clock(clock.clone())))
        .metrics(Arc::new(GatewayMetrics::default()))
}

fn advance(clock: &ManualClock, ms: u64) {
    clock.advance_to(Duration::from_millis(ms)).unwrap();
}

#[tokio::test]
async fn task_mutations_and_human_pause_use_one_clock_without_rewriting_imports() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let engine = TaskEngine::new(store).with_clock(clock.clone());
    let scope = TaskScope::new("ns", "tenant");
    let imported = Task::new_at(
        "task",
        "ns",
        "tenant",
        clock.now() - chrono::Duration::seconds(1),
    );
    let original = imported.created_at;
    assert_eq!(
        engine.create_task(imported).await.unwrap().created_at,
        original
    );
    advance(&clock, 10);
    let working = engine
        .transition_task(&scope, "task", TaskState::Working, None)
        .await
        .unwrap();
    assert_eq!(working.status.timestamp, clock.now());
    assert_eq!(working.last_progress_at, Some(clock.now()));
    advance(&clock, 20);
    let history = engine
        .append_history(
            &scope,
            "task",
            TaskMessage::text("message", TaskRole::User, "hello"),
        )
        .await
        .unwrap();
    assert_eq!(history.updated_at, clock.now());
    advance(&clock, 30);
    let artifact = engine
        .upsert_artifact(
            &scope,
            "task",
            Artifact::new("artifact", vec![TaskPart::text("one")]),
            false,
        )
        .await
        .unwrap();
    assert_eq!(artifact.last_progress_at, Some(clock.now()));
    advance(&clock, 40);
    let streamed = engine
        .apply_artifact_update(
            &scope,
            TaskArtifactUpdateEvent {
                task_id: "task".into(),
                context_id: None,
                artifact: Artifact::new("stream", vec![TaskPart::text("two")]),
                append: false,
                last_chunk: true,
                chunk_index: Some(0),
                total_chunks: Some(1),
                metadata: std::collections::HashMap::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(streamed.updated_at, clock.now());
    advance(&clock, 50);
    let heartbeat = engine.record_progress(&scope, "task").await.unwrap();
    assert_eq!(heartbeat.last_progress_at, Some(clock.now()));
    advance(&clock, 60);
    let linked = engine
        .link_to_chain(&scope, "task", Some("chain".into()))
        .await
        .unwrap();
    assert_eq!(linked.updated_at, clock.now());
    assert_eq!(
        linked.last_progress_at, heartbeat.last_progress_at,
        "administrative writes cannot renew liveness"
    );
    advance(&clock, 70);
    let (paused, approval) = engine
        .pause_for_human(
            &scope,
            "task",
            PauseKind::UserInput,
            None,
            Some(Duration::from_millis(100)),
        )
        .await
        .unwrap();
    assert_eq!(paused.status.timestamp, clock.now());
    assert_eq!(approval.created_at, clock.now());
    assert_eq!(
        approval.expires_at,
        clock.now() + chrono::Duration::milliseconds(100)
    );
}

#[tokio::test]
async fn polling_waits_on_manual_time_skips_missed_polls_and_cancels_timer_on_shutdown() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let key = StateKey::new("ns", "tenant", KeyKind::PendingChains, "chain");
    store
        .index_chain_ready(&key, clock.now().timestamp_millis())
        .await
        .unwrap();
    let (tx, mut rx) = mpsc::channel(16);
    let (mut worker, shutdown) = builder(&clock, &store)
        .config(BackgroundConfig {
            enable_group_flush: false,
            enable_timeout_processing: false,
            enable_silence_sync: false,
            enable_time_interval_sync: false,
            enable_group_sync: false,
            chain_check_interval: Duration::from_millis(100),
            ..Default::default()
        })
        .chain_advance_channel(tx)
        .build()
        .unwrap();
    let mut run = Box::pin(worker.run());
    assert!(poll!(&mut run).is_pending());
    assert_eq!(
        rx.try_recv().unwrap().chain_id,
        "chain",
        "first tick is immediate"
    );
    assert_eq!(clock.next_deadline(), Some(Duration::from_millis(100)));
    advance(&clock, 99);
    assert!(poll!(&mut run).is_pending());
    assert!(rx.try_recv().is_err());
    advance(&clock, 100);
    assert!(poll!(&mut run).is_pending());
    assert!(rx.try_recv().is_ok());
    advance(&clock, 10_000);
    assert!(poll!(&mut run).is_pending());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err(), "missed polls must not burst");
    assert_eq!(clock.next_deadline(), Some(Duration::from_millis(10_100)));
    advance(&clock, 10_100);
    shutdown.send(()).await.unwrap();
    assert!(
        poll!(&mut run).is_ready(),
        "shutdown wins over the due poll"
    );
    drop(run);
    assert!(rx.try_recv().is_err());
    assert_eq!(clock.pending_timers(), 0);
}

#[tokio::test]
async fn zero_enabled_period_is_rejected_but_disabled_jobs_are_safe() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    assert!(
        builder(&clock, &store)
            .config(BackgroundConfig {
                chain_check_interval: Duration::ZERO,
                ..Default::default()
            })
            .build()
            .is_err()
    );
    let (tx, mut rx) = mpsc::channel(1);
    let (mut worker, _shutdown) = builder(&clock, &store)
        .config(BackgroundConfig {
            enable_chain_advancement: false,
            chain_check_interval: Duration::ZERO,
            ..Default::default()
        })
        .chain_advance_channel(tx)
        .build()
        .unwrap();
    let key = StateKey::new("ns", "tenant", KeyKind::PendingChains, "chain");
    store
        .index_chain_ready(&key, clock.now().timestamp_millis())
        .await
        .unwrap();
    worker.tick(BackgroundJob::ChainAdvance).await.unwrap();
    assert!(rx.try_recv().is_err());
    assert_eq!(clock.pending_timers(), 0);
}

#[tokio::test]
async fn retention_cutoff_preserves_active_records_and_compliance_holds() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    for (tenant, held) in [("normal", false), ("held", true)] {
        let policy = json!({"id":tenant,"namespace":"ns","tenant":tenant,"enabled":true,
            "state_ttl_seconds":1,"event_ttl_seconds":1,"compliance_hold":held,
            "created_at":clock.now(),"updated_at":clock.now()});
        store
            .set(
                &StateKey::new("ns", tenant, KeyKind::Retention, tenant),
                &policy.to_string(),
                None,
            )
            .await
            .unwrap();
        for (id, state) in [("done", "resolved"), ("active", "open")] {
            store
                .set(
                    &StateKey::new("ns", tenant, KeyKind::EventState, id),
                    &json!({"state":state,"updated_at":clock.now()}).to_string(),
                    None,
                )
                .await
                .unwrap();
        }
        for (id, status) in [("done", "completed"), ("active", "running")] {
            store
                .set(
                    &StateKey::new("ns", tenant, KeyKind::Chain, id),
                    &json!({"status":status,"started_at":clock.now()}).to_string(),
                    None,
                )
                .await
                .unwrap();
        }
    }
    let (mut worker, _shutdown) = builder(&clock, &store)
        .config(BackgroundConfig {
            enable_retention_reaper: true,
            ..Default::default()
        })
        .build()
        .unwrap();
    for ms in [999, 1_000, 1_001] {
        advance(&clock, ms);
        worker.tick(BackgroundJob::Retention).await.unwrap();
        for tenant in ["normal", "held"] {
            for kind in [KeyKind::Chain, KeyKind::EventState] {
                assert_eq!(
                    store
                        .get(&StateKey::new("ns", tenant, kind.clone(), "done"))
                        .await
                        .unwrap()
                        .is_some(),
                    tenant == "held" || ms <= 1_000
                );
                assert!(
                    store
                        .get(&StateKey::new("ns", tenant, kind, "active"))
                        .await
                        .unwrap()
                        .is_some()
                );
            }
        }
    }
}

#[tokio::test]
async fn approval_retries_stop_at_expiry_even_when_the_record_is_retained() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let action = acteon_core::Action::new("ns", "tenant", "effect", "alert", json!({}));
    let row = json!({"action":action,"token":"approval","rule":"human","status":"pending",
        "created_at":clock.now(),"expires_at":clock.now()+chrono::Duration::seconds(1),
        "notification_sent":false});
    let key = StateKey::new("ns", "tenant", KeyKind::Approval, "approval");
    store.set(&key, &row.to_string(), None).await.unwrap();
    let (tx, mut rx) = mpsc::channel(8);
    let (mut worker, _shutdown) = builder(&clock, &store)
        .approval_retry_channel(tx)
        .build()
        .unwrap();
    for ms in [999, 1_000, 1_001] {
        advance(&clock, ms);
        worker.tick(BackgroundJob::Cleanup).await.unwrap();
        assert_eq!(rx.try_recv().is_ok(), ms == 999);
        assert!(store.get(&key).await.unwrap().is_some());
    }
}

#[tokio::test]
async fn recurring_tick_honors_due_time_end_time_and_advances_the_index() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let due = clock.now() + chrono::Duration::seconds(1);
    for (id, ends) in [("active", None), ("ending", Some(due))] {
        let row = json!({"id":id,"namespace":"ns","tenant":"tenant","cron_expr":"* * * * *",
            "action_template":{"provider":"effect","action_type":"alert","payload":{}},
            "created_at":clock.now(),"updated_at":clock.now(),"ends_at":ends});
        // Deserialize before storage so fixture drift cannot silently skip the worker path.
        let recurring: acteon_core::RecurringAction = serde_json::from_value(row).unwrap();
        store
            .set(
                &StateKey::new("ns", "tenant", KeyKind::RecurringAction, id),
                &serde_json::to_string(&recurring).unwrap(),
                None,
            )
            .await
            .unwrap();
        acteon_state::set_pending_recurring(
            store.as_ref(),
            &StateKey::new("ns", "tenant", KeyKind::PendingRecurring, id),
            due.timestamp_millis(),
        )
        .await
        .unwrap();
    }
    let (tx, mut rx) = mpsc::channel(8);
    let (mut worker, _shutdown) = builder(&clock, &store)
        .recurring_action_channel(tx)
        .config(BackgroundConfig {
            enable_recurring_actions: true,
            ..Default::default()
        })
        .build()
        .unwrap();
    advance(&clock, 999);
    worker.tick(BackgroundJob::RecurringActions).await.unwrap();
    assert!(rx.try_recv().is_err());
    advance(&clock, 1_000);
    worker.tick(BackgroundJob::RecurringActions).await.unwrap();
    assert_eq!(rx.try_recv().unwrap().recurring_id, "active");
    assert!(
        rx.try_recv().is_err(),
        "ends_at excludes the exact deadline"
    );
    assert!(
        store
            .get_expired_timeouts(due.timestamp_millis())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .get(&StateKey::new(
                "ns",
                "tenant",
                KeyKind::PendingRecurring,
                "ending"
            ))
            .await
            .unwrap()
            .is_none()
    );
    let next = store
        .get(&StateKey::new(
            "ns",
            "tenant",
            KeyKind::PendingRecurring,
            "active",
        ))
        .await
        .unwrap()
        .unwrap();
    let expected = chrono::DateTime::from_timestamp(1_700_000_040, 0).unwrap();
    assert_eq!(next.parse::<i64>().unwrap(), expected.timestamp_millis());
    advance(&clock, 1_001);
    worker.tick(BackgroundJob::RecurringActions).await.unwrap();
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn dropping_poll_loop_unregisters_its_wait() {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let (mut worker, _shutdown) = builder(&clock, &store).build().unwrap();
    let mut run = Box::pin(worker.run());
    assert!(poll!(&mut run).is_pending());
    assert_eq!(clock.pending_timers(), 1);
    drop(run);
    assert_eq!(clock.pending_timers(), 0);
}
