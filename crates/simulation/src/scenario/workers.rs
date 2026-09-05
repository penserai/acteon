//! Manual-clock evidence for production task mutations and worker cycles.

use std::sync::Arc;
use std::time::Duration;

use acteon_audit::{AuditQuery, store::AuditStore};
use acteon_audit_memory::MemoryAuditStore;
use acteon_core::{Action, PauseKind, Task, TaskState};
use acteon_gateway::task_engine::{TaskEngine, TaskScope};
use acteon_gateway::{
    BackgroundConfig, BackgroundJob, BackgroundProcessorBuilder, GatewayMetrics, GroupManager,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use acteon_time::{Clock, ManualClock};
use futures::poll;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use super::{Scenario, ScenarioReport, evaluation::derived_seed};
use crate::SimulationError;

const SCENARIO: Scenario = Scenario::WorkerLifecycle;

fn error(error: impl std::fmt::Display) -> SimulationError {
    SimulationError::Gateway(error.to_string())
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("fixed epoch"),
    ))
}

fn advance(clock: &ManualClock, ms: u64) -> Result<(), SimulationError> {
    clock.advance_to(Duration::from_millis(ms)).map_err(error)
}

fn builder(clock: &Arc<ManualClock>, store: &Arc<MemoryStateStore>) -> BackgroundProcessorBuilder {
    BackgroundProcessorBuilder::new()
        .clock(clock.clone())
        .state(store.clone())
        .group_manager(Arc::new(GroupManager::with_clock(clock.clone())))
        .metrics(Arc::new(GatewayMetrics::default()))
}

fn record(report: &mut ScenarioReport, name: &str, passed: bool, evidence: &serde_json::Value) {
    let detail = evidence.to_string();
    report.check(SCENARIO, name, passed, &detail);
    report.event(SCENARIO, name, &detail, 0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    FrozenTaskClock,
    FrozenWorkerClock,
}

pub(super) async fn run(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    run_with(report, Mutation::None).await
}

async fn run_with(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    tasks(report, mutation).await?;
    due_ticks(report, mutation).await?;
    polling(report).await
}

// Keep the ordered lifecycle and its observations together.
#[allow(clippy::too_many_lines)]
async fn tasks(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let clock = clock();
    let epoch = clock.now();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let audit = Arc::new(MemoryAuditStore::new());
    let (stream, mut events) = broadcast::channel(32);
    let engine_clock = if mutation == Mutation::FrozenTaskClock {
        Arc::new(ManualClock::new(epoch))
    } else {
        clock.clone()
    };
    let engine = TaskEngine::new(store.clone())
        .with_clock(engine_clock)
        .with_audit(audit.clone());
    let scope = TaskScope::new("workers", "alice");
    let ttl = 100 + derived_seed(report.manifest.seed, 0, "task_working_ttl") % 100;
    for id in ["stale", "alive", "done"] {
        let task = Task::new_at(id, "workers", "alice", epoch)
            .with_working_ttl(i64::try_from(ttl).expect("bounded TTL"))
            .map_err(error)?;
        engine.create_task(task).await.map_err(error)?;
        engine
            .transition_task(&scope, id, TaskState::Working, None)
            .await
            .map_err(error)?;
    }
    engine
        .transition_task(&scope, "done", TaskState::Completed, None)
        .await
        .map_err(error)?;
    advance(&clock, 50)?;
    let (paused, approval) = engine
        .pause_for_human(
            &scope,
            "stale",
            PauseKind::UserInput,
            None,
            Some(Duration::from_millis(500)),
        )
        .await
        .map_err(error)?;
    advance(&clock, 60)?;
    let linked = engine
        .link_to_chain(&scope, "stale", Some("chain".into()))
        .await
        .map_err(error)?;
    record(
        report,
        "task_timestamps",
        paused.status.timestamp == epoch + chrono::Duration::milliseconds(50)
            && approval.created_at == paused.status.timestamp
            && approval.expires_at == epoch + chrono::Duration::milliseconds(550)
            && linked.updated_at == clock.now()
            && linked.last_progress_at == Some(paused.status.timestamp),
        &json!({"pause_at":paused.status.timestamp,"approval_expires_at":approval.expires_at,
            "administrative_update_at":linked.updated_at,"last_progress_at":linked.last_progress_at}),
    );
    let worker_clock = if mutation == Mutation::FrozenWorkerClock {
        Arc::new(ManualClock::new(epoch))
    } else {
        clock.clone()
    };
    let (mut worker, _shutdown) = builder(&worker_clock, &store)
        .audit(audit.clone())
        .stream_tx(stream)
        .config(BackgroundConfig {
            enable_stale_task_reaper: true,
            ..Default::default()
        })
        .build()
        .map_err(error)?;
    let mut states = Vec::new();
    // Heartbeat at the old deadline, before the explicitly ordered reaper tick.
    advance(&clock, 49 + ttl)?;
    engine
        .record_progress(&scope, "alive")
        .await
        .map_err(error)?;
    for at in [49 + ttl, 50 + ttl, 51 + ttl, 52 + ttl] {
        advance(&clock, at)?;
        worker
            .tick(BackgroundJob::StaleTasks)
            .await
            .map_err(error)?;
        let task = engine
            .get_task(&scope, "stale")
            .await
            .map_err(error)?
            .expect("created task");
        states.push(task.status.state);
    }
    let stale = engine
        .get_task(&scope, "stale")
        .await
        .map_err(error)?
        .expect("created task");
    let alive = engine
        .get_task(&scope, "alive")
        .await
        .map_err(error)?
        .expect("created task");
    let done = engine
        .get_task(&scope, "done")
        .await
        .map_err(error)?
        .expect("created task");
    let mut stream_times = Vec::new();
    while let Ok(event) = events.try_recv() {
        stream_times.push(event.timestamp);
    }
    let page = audit
        .query(&AuditQuery {
            namespace: Some("workers".into()),
            tenant: Some("alice".into()),
            ..Default::default()
        })
        .await
        .map_err(error)?;
    let mut reap_times: Vec<_> = page
        .records
        .iter()
        .filter(|row| {
            row.outcome_details
                .get("operation")
                .is_some_and(|op| op == "reap")
        })
        .map(|row| row.dispatched_at)
        .collect();
    reap_times.sort();
    let expected_reap =
        epoch + chrono::Duration::milliseconds(i64::try_from(51 + ttl).expect("bounded time"));
    record(
        report,
        "task_reaping",
        states
            == [
                TaskState::InputRequired,
                TaskState::InputRequired,
                TaskState::Failed,
                TaskState::Failed,
            ]
            && alive.status.state == TaskState::Working
            && done.status.state == TaskState::Completed
            && stale.status.timestamp == expected_reap
            && stream_times == [expected_reap]
            && reap_times == [expected_reap],
        &json!({"ttl_ms":ttl,"states":states,"alive":alive.status.state,"terminal":done.status.state,
            "reaped_at":stale.status.timestamp,"stream_times":stream_times,"audit_times":reap_times}),
    );
    Ok(())
}

async fn due_ticks(report: &mut ScenarioReport, mutation: Mutation) -> Result<(), SimulationError> {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let groups = Arc::new(GroupManager::with_clock(clock.clone()));
    let mut action = Action::new("workers", "alice", "effect", "alert", json!({}));
    action.created_at = clock.now();
    groups
        .add_to_group(&action, &[], 1, 5, None, 10, store.as_ref(), None)
        .await
        .map_err(error)?;
    let due = clock.now().timestamp_millis() + 1_000;
    let event = StateKey::new("workers", "alice", KeyKind::EventTimeout, "event");
    store
        .set(
            &event,
            &json!({"state_machine":"alert","current_state":"open","transition_to":"resolved"})
                .to_string(),
            None,
        )
        .await
        .map_err(error)?;
    store.index_timeout(&event, due).await.map_err(error)?;
    let scheduled = StateKey::new("workers", "alice", KeyKind::ScheduledAction, "scheduled");
    store
        .set(&scheduled, &json!({"action_id":"scheduled","action":action,"scheduled_for":clock.now()+chrono::Duration::seconds(1),"created_at":clock.now()}).to_string(), None)
        .await
        .map_err(error)?;
    let pending = StateKey::new("workers", "alice", KeyKind::PendingScheduled, "scheduled");
    store
        .set(&pending, &due.to_string(), None)
        .await
        .map_err(error)?;
    store.index_timeout(&pending, due).await.map_err(error)?;
    let (group_tx, mut group_rx) = mpsc::channel(8);
    let (timeout_tx, mut timeout_rx) = mpsc::channel(8);
    let (scheduled_tx, mut scheduled_rx) = mpsc::channel(8);
    let worker_clock = if mutation == Mutation::FrozenWorkerClock {
        Arc::new(ManualClock::new(clock.now()))
    } else {
        clock.clone()
    };
    let (mut worker, _shutdown) = builder(&worker_clock, &store)
        .group_manager(groups)
        .group_flush_channel(group_tx)
        .timeout_channel(timeout_tx)
        .scheduled_action_channel(scheduled_tx)
        .config(BackgroundConfig {
            enable_scheduled_actions: true,
            ..Default::default()
        })
        .build()
        .map_err(error)?;
    let mut observations = Vec::new();
    let mut correct_times = true;
    for ms in [999, 1_000, 1_001] {
        advance(&clock, ms)?;
        for job in [
            BackgroundJob::GroupFlush,
            BackgroundJob::Timeout,
            BackgroundJob::ScheduledActions,
        ] {
            worker.tick(job).await.map_err(error)?;
        }
        let groups: Vec<_> = std::iter::from_fn(|| group_rx.try_recv().ok()).collect();
        let timeouts: Vec<_> = std::iter::from_fn(|| timeout_rx.try_recv().ok()).collect();
        let scheduled: Vec<_> = std::iter::from_fn(|| scheduled_rx.try_recv().ok()).collect();
        correct_times &= groups.iter().all(|event| event.flushed_at == clock.now())
            && timeouts.iter().all(|event| event.fired_at == clock.now());
        observations.push((ms, groups.len(), timeouts.len(), scheduled.len()));
    }
    record(
        report,
        "due_ticks",
        correct_times && observations == [(999, 0, 0, 0), (1_000, 1, 1, 1), (1_001, 0, 0, 0)],
        &json!({"ticks":observations,"timestamps_match":correct_times}),
    );
    Ok(())
}

async fn polling(report: &mut ScenarioReport) -> Result<(), SimulationError> {
    let clock = clock();
    let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
    let key = StateKey::new("workers", "alice", KeyKind::PendingChains, "chain");
    store
        .index_chain_ready(&key, clock.now().timestamp_millis())
        .await
        .map_err(error)?;
    let (tx, mut rx) = mpsc::channel(64);
    let (mut worker, shutdown) = builder(&clock, &store)
        .chain_advance_channel(tx)
        .config(BackgroundConfig {
            enable_group_flush: false,
            enable_timeout_processing: false,
            enable_silence_sync: false,
            enable_time_interval_sync: false,
            enable_group_sync: false,
            chain_check_interval: Duration::from_millis(100),
            ..Default::default()
        })
        .build()
        .map_err(error)?;
    // Own and poll the loop in this root future; no spawned task or OS timer.
    let mut run = Box::pin(tokio::task::unconstrained(worker.run()));
    let mut counts = Vec::new();
    let mut pending = true;
    for ms in [0, 99, 100, 10_000] {
        advance(&clock, ms)?;
        pending &= poll!(&mut run).is_pending();
        counts.push(std::iter::from_fn(|| rx.try_recv().ok()).count());
    }
    let next = clock.next_deadline();
    advance(&clock, 10_100)?;
    shutdown.send(()).await.map_err(error)?;
    let stopped = poll!(&mut run).is_ready();
    drop(run);
    let shutdown_events = std::iter::from_fn(|| rx.try_recv().ok()).count();
    record(
        report,
        "polling_clock",
        pending
            && counts == [1, 0, 1, 1]
            && next == Some(Duration::from_millis(10_100))
            && stopped
            && shutdown_events == 0
            && clock.pending_timers() == 0,
        &json!({"events_per_poll":counts,"next_ms":next.map(|d|d.as_millis()),
            "stopped":stopped,"shutdown_events":shutdown_events,"pending_timers":clock.pending_timers()}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Backend, ScenarioManifest, evaluation};
    use super::*;

    #[tokio::test]
    async fn worker_evidence_replays_and_clock_mutations_fail_safety_gates() {
        let manifest = ScenarioManifest {
            schema_version: 1,
            seed: 42,
            backend: Backend::Memory,
            scenarios: vec![SCENARIO],
        };
        let baseline = super::super::run(manifest.clone()).await.unwrap();
        assert!(baseline.passed(), "{:?}", baseline.invariants);
        let replay = super::super::run(manifest).await.unwrap();
        assert!(replay.same_evidence(&baseline));
        for (mutation, failed) in [
            (Mutation::FrozenTaskClock, "task_timestamps"),
            (Mutation::FrozenWorkerClock, "task_reaping"),
        ] {
            let mut report = super::super::run(baseline.manifest.clone()).await.unwrap();
            report.invariants.clear();
            report.trace.clear();
            run_with(&mut report, mutation).await.unwrap();
            assert!(
                report
                    .invariants
                    .iter()
                    .any(|check| check.name == failed && !check.passed)
            );
            let score = evaluation::grade(SCENARIO, &report);
            assert!(!score.passed);
            assert!(score.gates.iter().any(|gate| !gate.passed));
        }
    }
}
