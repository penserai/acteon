use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::SwarmProviderError;
use crate::executor::SharedExecutor;
use crate::sink::CompletionSink;
use crate::types::{
    GoalRequest, SwarmGoalAccepted, SwarmRunFilter, SwarmRunSnapshot, SwarmRunStatus,
};

/// A live run tracked by the registry.
pub struct SwarmRunHandle {
    pub snapshot: SwarmRunSnapshot,
    cancel: CancellationToken,
}

impl SwarmRunHandle {
    /// Whether this run currently holds a capacity slot. A run stops
    /// holding a slot the moment it transitions to a terminal status OR
    /// to `Cancelling` — once we've asked it to stop, a fresh goal can
    /// take its place.
    #[must_use]
    pub fn holds_capacity_slot(&self) -> bool {
        holds_capacity_slot(&self.snapshot.status)
    }
}

fn holds_capacity_slot(status: &SwarmRunStatus) -> bool {
    !status.is_terminal() && *status != SwarmRunStatus::Cancelling
}

/// Shared registry of swarm runs.
///
/// Holds per-run `CancellationToken`s plus snapshots of the latest status,
/// exposes list/get/cancel APIs used by both the provider and the HTTP layer.
pub struct SwarmRunRegistry {
    runs: DashMap<String, SwarmRunHandle>,
    sink: Arc<dyn CompletionSink>,
    executor: SharedExecutor,
    max_concurrent_runs: usize,
    idempotency_index: DashMap<String, String>,
    /// Broadcast channel for every status transition. Subscribers (SSE
    /// handlers, rule reactors, external observers) can hang a receiver
    /// off this to react in real time without polling.
    updates_tx: broadcast::Sender<SwarmRunSnapshot>,
    /// O(1) inflight counter. A run is "inflight" while it holds a capacity
    /// slot — i.e. after `start()` and before the first transition into a
    /// terminal or `Cancelling` status.
    inflight_count: AtomicUsize,
}

/// Hard cap on how many snapshots a single `list()` call can return.
/// Safeguards the server against unbounded pages at the registry layer
/// regardless of what the HTTP surface does.
pub const MAX_LIST_PAGE: usize = 500;

impl SwarmRunRegistry {
    #[must_use]
    pub fn new(
        executor: SharedExecutor,
        sink: Arc<dyn CompletionSink>,
        max_concurrent_runs: usize,
    ) -> Arc<Self> {
        let (updates_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            runs: DashMap::new(),
            sink,
            executor,
            max_concurrent_runs: max_concurrent_runs.max(1),
            idempotency_index: DashMap::new(),
            updates_tx,
            inflight_count: AtomicUsize::new(0),
        })
    }

    /// Subscribe to the stream of status transitions. Each subscriber
    /// receives every update pushed after the subscription is taken.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SwarmRunSnapshot> {
        self.updates_tx.subscribe()
    }

    /// Count of inflight runs (holding a capacity slot). O(1).
    #[must_use]
    pub fn inflight(&self) -> usize {
        self.inflight_count.load(Ordering::Acquire)
    }

    /// Total runs retained in the registry (including terminal ones pending
    /// reaping). Exposed for metrics and tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether the registry currently tracks zero runs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Accept a goal and start it in the background. Returns the accepted
    /// receipt immediately; the actual execution progresses asynchronously.
    pub async fn start(
        self: &Arc<Self>,
        namespace: String,
        tenant: String,
        request: GoalRequest,
    ) -> Result<SwarmGoalAccepted, SwarmProviderError> {
        if let Some(key) = &request.idempotency_key
            && let Some(existing) = self.idempotency_index.get(key)
            && let Some(handle) = self.runs.get(existing.value())
        {
            return Ok(SwarmGoalAccepted {
                run_id: handle.snapshot.run_id.clone(),
                plan_id: handle.snapshot.plan_id.clone(),
                started_at: handle.snapshot.started_at,
                objective: handle.snapshot.objective.clone(),
            });
        }

        // Reserve a capacity slot atomically so concurrent dispatches don't
        // over-commit past `max_concurrent_runs`.
        loop {
            let current = self.inflight_count.load(Ordering::Acquire);
            if current >= self.max_concurrent_runs {
                return Err(SwarmProviderError::RegistryFull {
                    max: self.max_concurrent_runs,
                });
            }
            if self
                .inflight_count
                .compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now();
        let cancel = CancellationToken::new();

        let snapshot = SwarmRunSnapshot {
            run_id: run_id.clone(),
            plan_id: request.plan.id.clone(),
            objective: request.objective.clone(),
            status: SwarmRunStatus::Accepted,
            started_at,
            finished_at: None,
            metrics: None,
            error: None,
            namespace: namespace.clone(),
            tenant: tenant.clone(),
        };

        self.runs.insert(
            run_id.clone(),
            SwarmRunHandle {
                snapshot: snapshot.clone(),
                cancel: cancel.clone(),
            },
        );

        if let Some(key) = &request.idempotency_key {
            self.idempotency_index.insert(key.clone(), run_id.clone());
        }

        let _ = self.updates_tx.send(snapshot.clone());
        self.sink.on_status(&snapshot).await;

        let this = self.clone();
        let plan_id = request.plan.id.clone();
        let objective = request.objective.clone();
        let plan = request.plan;
        let id_for_task = run_id.clone();
        tokio::spawn(async move {
            this.transition(&id_for_task, SwarmRunStatus::Running, None, None)
                .await;
            let result = tokio::select! {
                r = this.executor.run(plan) => r,
                () = cancel.cancelled() => {
                    this.transition(&id_for_task, SwarmRunStatus::Cancelled, None, None).await;
                    return;
                }
            };
            match result {
                Ok(run) => {
                    let status = run.status.clone().into();
                    this.transition(&id_for_task, status, Some(run.metrics.clone()), None)
                        .await;
                }
                Err(err) => {
                    this.transition(
                        &id_for_task,
                        SwarmRunStatus::Failed,
                        None,
                        Some(err.to_string()),
                    )
                    .await;
                }
            }
        });

        Ok(SwarmGoalAccepted {
            run_id,
            plan_id,
            started_at,
            objective,
        })
    }

    async fn transition(
        &self,
        run_id: &str,
        status: SwarmRunStatus,
        metrics: Option<acteon_swarm::types::run::RunMetrics>,
        error: Option<String>,
    ) {
        let outcome = {
            let Some(mut entry) = self.runs.get_mut(run_id) else {
                return;
            };
            // Terminal states are absorbing. Prevents the cancel foreground
            // path from overwriting a `Cancelled`/`Completed`/... snapshot
            // written by the background task (and vice versa).
            if entry.snapshot.status.is_terminal() {
                return;
            }
            // No-op on identical transitions (e.g. repeated cancel() calls).
            if entry.snapshot.status == status {
                return;
            }
            let was_holding_slot = holds_capacity_slot(&entry.snapshot.status);
            entry.snapshot.status = status.clone();
            if status.is_terminal() {
                entry.snapshot.finished_at = Some(Utc::now());
            }
            if let Some(m) = metrics {
                entry.snapshot.metrics = Some(m);
            }
            if let Some(err) = error {
                entry.snapshot.error = Some(err);
            }
            let now_holds_slot = holds_capacity_slot(&status);
            let released_slot = was_holding_slot && !now_holds_slot;
            (entry.snapshot.clone(), released_slot)
        };
        let (snapshot, released_slot) = outcome;
        if released_slot {
            self.inflight_count.fetch_sub(1, Ordering::AcqRel);
        }
        let _ = self.updates_tx.send(snapshot.clone());
        self.sink.on_status(&snapshot).await;
    }

    /// Fetch a snapshot by run ID.
    #[must_use]
    pub fn get(&self, run_id: &str) -> Option<SwarmRunSnapshot> {
        self.runs.get(run_id).map(|h| h.snapshot.clone())
    }

    /// List snapshots matching the filter, ordered newest first. Returns a
    /// `(page, total)` tuple where `total` is the filter-matching count
    /// *before* pagination so callers can render correct page counts.
    ///
    /// The returned page is always capped at [`MAX_LIST_PAGE`] regardless
    /// of `filter.limit` to protect the server from unbounded response
    /// bodies.
    #[must_use]
    pub fn list(&self, filter: &SwarmRunFilter) -> (Vec<SwarmRunSnapshot>, usize) {
        let mut out: Vec<SwarmRunSnapshot> = self
            .runs
            .iter()
            .map(|e| e.value().snapshot.clone())
            .filter(|s| {
                filter
                    .namespace
                    .as_deref()
                    .is_none_or(|ns| ns == s.namespace)
                    && filter.tenant.as_deref().is_none_or(|t| t == s.tenant)
                    && filter.status.as_ref().is_none_or(|st| st == &s.status)
            })
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        let total = out.len();
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(MAX_LIST_PAGE).min(MAX_LIST_PAGE);
        (out.into_iter().skip(offset).take(limit).collect(), total)
    }

    /// Remove terminal runs that finished more than `max_age` ago.
    /// Returns the number of runs evicted.
    ///
    /// Idempotency index entries pointing at evicted runs are cleaned up
    /// in the same pass. Callers typically invoke this on an interval via
    /// [`Self::spawn_reaper`].
    pub fn reap_terminal_older_than(&self, max_age: Duration) -> usize {
        let Ok(age) = chrono::Duration::from_std(max_age) else {
            return 0;
        };
        let threshold = Utc::now() - age;
        let to_remove: Vec<String> = self
            .runs
            .iter()
            .filter_map(|e| {
                let snap = &e.value().snapshot;
                if snap.status.is_terminal() && snap.finished_at.is_some_and(|f| f < threshold) {
                    Some(e.key().clone())
                } else {
                    None
                }
            })
            .collect();
        let evicted = to_remove.len();
        for id in &to_remove {
            self.runs.remove(id);
        }
        // Drop idempotency keys that now point at evicted runs.
        self.idempotency_index
            .retain(|_, run_id| self.runs.contains_key(run_id));
        evicted
    }

    /// Spawn a background task that reaps terminal runs older than
    /// `retention` every `interval`. The task runs for as long as the
    /// returned `JoinHandle` is alive.
    #[must_use]
    pub fn spawn_reaper(
        self: &Arc<Self>,
        retention: Duration,
        interval: Duration,
    ) -> JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            // Skip the immediate tick so we don't reap at startup.
            timer.tick().await;
            loop {
                timer.tick().await;
                let evicted = this.reap_terminal_older_than(retention);
                if evicted > 0 {
                    tracing::info!(evicted, "swarm registry reaper evicted terminal runs");
                }
            }
        })
    }

    /// Request cancellation of a run. Returns `NotFound` if the id is unknown,
    /// succeeds even if the run is already terminal (idempotent).
    ///
    /// The foreground path transitions the run to `Cancelling`; the
    /// background task observes the cancellation token and races to a
    /// terminal `Cancelled` snapshot. Terminal states are absorbing, so
    /// whichever path sets a terminal status first wins — neither can be
    /// overwritten.
    pub async fn cancel(&self, run_id: &str) -> Result<SwarmRunSnapshot, SwarmProviderError> {
        let (cancel, holds_slot) = {
            let Some(handle) = self.runs.get(run_id) else {
                return Err(SwarmProviderError::NotFound(run_id.into()));
            };
            (handle.cancel.clone(), handle.holds_capacity_slot())
        };
        if holds_slot {
            cancel.cancel();
            self.transition(run_id, SwarmRunStatus::Cancelling, None, None)
                .await;
        }
        self.get(run_id)
            .ok_or_else(|| SwarmProviderError::NotFound(run_id.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::SwarmExecutor;
    use crate::sink::NoopSink;
    use acteon_swarm::SwarmPlan;
    use acteon_swarm::types::plan::SwarmScope;
    use acteon_swarm::types::run::{RunMetrics, SwarmRun, SwarmRunStatus as InnerStatus};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct DelayExecutor {
        delay_ms: u64,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SwarmExecutor for DelayExecutor {
        async fn run(&self, plan: SwarmPlan) -> Result<SwarmRun, SwarmProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(SwarmRun {
                id: uuid::Uuid::new_v4().to_string(),
                plan_id: plan.id.clone(),
                status: InnerStatus::Completed,
                started_at: chrono::Utc::now(),
                finished_at: Some(chrono::Utc::now()),
                task_status: std::collections::HashMap::new(),
                metrics: RunMetrics::default(),
            })
        }
    }

    fn sample_plan() -> SwarmPlan {
        SwarmPlan {
            id: "plan-test".into(),
            objective: "demo".into(),
            scope: SwarmScope::default(),
            success_criteria: vec![],
            tasks: vec![],
            agent_roles: vec![],
            estimated_actions: 0,
            created_at: chrono::Utc::now(),
            approved_at: None,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn start_returns_accepted_and_transitions_to_completed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            8,
        );
        let accepted = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(accepted.plan_id, "plan-test");

        // Let the spawned task finish.
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;

        let snap = reg.get(&accepted.run_id).expect("snapshot");
        assert_eq!(snap.status, SwarmRunStatus::Completed);
        assert!(snap.finished_at.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn idempotency_collapses_to_same_run() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10_000,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            8,
        );
        let req = || GoalRequest {
            objective: "demo".into(),
            plan: sample_plan(),
            idempotency_key: Some("key-a".into()),
        };
        let a = reg.start("ns".into(), "t".into(), req()).await.unwrap();
        let b = reg.start("ns".into(), "t".into(), req()).await.unwrap();
        assert_eq!(a.run_id, b.run_id);
    }

    #[tokio::test(start_paused = true)]
    async fn registry_full_when_capacity_exceeded() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10_000,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            1,
        );
        reg.start(
            "ns".into(),
            "t".into(),
            GoalRequest {
                objective: "demo".into(),
                plan: sample_plan(),
                idempotency_key: None,
            },
        )
        .await
        .unwrap();
        // Yield so the spawned task flips status to Running.
        tokio::task::yield_now().await;
        let err = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SwarmProviderError::RegistryFull { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_flips_to_cancelled() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10_000,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            8,
        );
        let accepted = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap();
        tokio::task::yield_now().await;
        let snap = reg.cancel(&accepted.run_id).await.unwrap();
        assert_eq!(snap.status, SwarmRunStatus::Cancelling);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        let snap = reg.get(&accepted.run_id).unwrap();
        assert_eq!(snap.status, SwarmRunStatus::Cancelled);
    }

    #[tokio::test]
    async fn subscribe_observes_transitions() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 0,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            4,
        );
        let mut rx = reg.subscribe();
        let accepted = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap();

        // First event must be the Accepted transition.
        let first = rx.recv().await.unwrap();
        assert_eq!(first.run_id, accepted.run_id);
        assert_eq!(first.status, SwarmRunStatus::Accepted);

        // Walk forward until terminal.
        let mut seen_terminal = false;
        for _ in 0..6 {
            match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
                Ok(Ok(snap)) => {
                    if snap.status.is_terminal() {
                        seen_terminal = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(seen_terminal, "expected to observe a terminal status");
    }

    #[tokio::test]
    async fn cancel_unknown_run_is_not_found() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 0,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            8,
        );
        let err = reg.cancel("missing").await.unwrap_err();
        assert!(matches!(err, SwarmProviderError::NotFound(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn inflight_counter_returns_to_zero_after_completion() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            4,
        );
        assert_eq!(reg.inflight(), 0);
        let _ = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap();
        // Slot reserved synchronously on start.
        assert_eq!(reg.inflight(), 1);
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
        assert_eq!(reg.inflight(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn capacity_frees_up_after_terminal_transition() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 5,
                calls: calls.clone(),
            }),
            Arc::new(NoopSink),
            1,
        );
        for _ in 0..3 {
            reg.start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .expect("dispatch should succeed once the prior slot frees up");
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(reg.inflight(), 0);
    }

    #[tokio::test]
    async fn terminal_status_is_absorbing() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 0,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            4,
        );
        // Insert a synthetic handle already in a terminal state.
        let run_id = "run-already-terminal".to_string();
        reg.runs.insert(
            run_id.clone(),
            SwarmRunHandle {
                snapshot: SwarmRunSnapshot {
                    run_id: run_id.clone(),
                    plan_id: "p".into(),
                    objective: "o".into(),
                    status: SwarmRunStatus::Completed,
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    metrics: None,
                    error: None,
                    namespace: "ns".into(),
                    tenant: "t".into(),
                },
                cancel: CancellationToken::new(),
            },
        );

        // Attempt every non-terminal transition — all must be ignored.
        for attempted in [
            SwarmRunStatus::Running,
            SwarmRunStatus::Cancelling,
            SwarmRunStatus::Failed,
            SwarmRunStatus::Cancelled,
        ] {
            reg.transition(&run_id, attempted, None, None).await;
            assert_eq!(
                reg.get(&run_id).unwrap().status,
                SwarmRunStatus::Completed,
                "terminal status must not be overwritten",
            );
        }
    }

    #[tokio::test]
    async fn reaper_evicts_old_terminal_runs() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 0,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            4,
        );
        // Seed two terminal runs: one old, one fresh.
        let old = "old".to_string();
        let fresh = "fresh".to_string();
        let long_ago = chrono::Utc::now() - chrono::Duration::seconds(120);
        reg.runs.insert(
            old.clone(),
            SwarmRunHandle {
                snapshot: SwarmRunSnapshot {
                    run_id: old.clone(),
                    plan_id: "p".into(),
                    objective: "o".into(),
                    status: SwarmRunStatus::Completed,
                    started_at: long_ago,
                    finished_at: Some(long_ago),
                    metrics: None,
                    error: None,
                    namespace: "ns".into(),
                    tenant: "t".into(),
                },
                cancel: CancellationToken::new(),
            },
        );
        reg.runs.insert(
            fresh.clone(),
            SwarmRunHandle {
                snapshot: SwarmRunSnapshot {
                    run_id: fresh.clone(),
                    plan_id: "p".into(),
                    objective: "o".into(),
                    status: SwarmRunStatus::Completed,
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    metrics: None,
                    error: None,
                    namespace: "ns".into(),
                    tenant: "t".into(),
                },
                cancel: CancellationToken::new(),
            },
        );
        reg.idempotency_index.insert("old-key".into(), old.clone());
        reg.idempotency_index
            .insert("fresh-key".into(), fresh.clone());

        let evicted = reg.reap_terminal_older_than(Duration::from_secs(60));
        assert_eq!(evicted, 1);
        assert!(reg.get(&old).is_none());
        assert!(reg.get(&fresh).is_some());
        assert!(!reg.idempotency_index.contains_key("old-key"));
        assert!(reg.idempotency_index.contains_key("fresh-key"));
    }

    #[tokio::test(start_paused = true)]
    async fn reaper_skips_inflight_runs() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 10_000,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            4,
        );
        let accepted = reg
            .start(
                "ns".into(),
                "t".into(),
                GoalRequest {
                    objective: "demo".into(),
                    plan: sample_plan(),
                    idempotency_key: None,
                },
            )
            .await
            .unwrap();
        // Even with a zero retention the reaper must not touch inflight runs.
        let evicted = reg.reap_terminal_older_than(Duration::from_secs(0));
        assert_eq!(evicted, 0);
        assert!(reg.get(&accepted.run_id).is_some());
    }

    #[tokio::test]
    async fn list_returns_full_total_and_clamps_page() {
        let reg = SwarmRunRegistry::new(
            Arc::new(DelayExecutor {
                delay_ms: 0,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoopSink),
            1024,
        );
        for i in 0..5 {
            reg.runs.insert(
                format!("r{i}"),
                SwarmRunHandle {
                    snapshot: SwarmRunSnapshot {
                        run_id: format!("r{i}"),
                        plan_id: "p".into(),
                        objective: "o".into(),
                        status: SwarmRunStatus::Completed,
                        started_at: chrono::Utc::now() + chrono::Duration::seconds(i),
                        finished_at: None,
                        metrics: None,
                        error: None,
                        namespace: "ns".into(),
                        tenant: "t".into(),
                    },
                    cancel: CancellationToken::new(),
                },
            );
        }
        // Request a page of 2 — page reflects the cap, total reflects the
        // true match count.
        let (page, total) = reg.list(&SwarmRunFilter {
            limit: Some(2),
            ..Default::default()
        });
        assert_eq!(page.len(), 2);
        assert_eq!(total, 5);
    }
}
