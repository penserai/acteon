//! Recovery reads immutable acceptance; all new effects still register at the
//! governed executor's authority/budget CAS. A task projection is never a queue.
use super::ExecutionAuthorityRuntime;
use crate::config::AgentServiceDriverConfig;
use acteon_executor::governed::GovernedProviderStatus;
use acteon_gateway::agent_runtime::{ACCEPTANCE_KIND, AgentProviderRuntime};
use acteon_state::KeyKind;
use futures::FutureExt;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type WorkId = (String, String, Uuid);
type Work = (Arc<AgentProviderRuntime>, WorkId);
type Cursors = BTreeMap<(String, String), String>;

pub struct AgentServiceDriver {
    stop: CancellationToken,
    task: JoinHandle<()>,
}
impl AgentServiceDriver {
    /// Stop taking work, allow bounded completion, then retain any ambiguous
    /// started work as unresolved. Aborting a future is never a cancellation ACK.
    pub async fn shutdown(mut self, timeout: Duration) {
        self.stop.cancel();
        if tokio::time::timeout(timeout, &mut self.task).await.is_err() {
            self.task.abort();
            let _ = self.task.await;
        }
    }
}
impl ExecutionAuthorityRuntime {
    pub fn spawn_agent_driver(
        self: &Arc<Self>,
        config: &AgentServiceDriverConfig,
    ) -> Result<Option<AgentServiceDriver>, String> {
        config.validate()?;
        if !config.enabled
            || self
                .scopes
                .values()
                .all(|scope| scope.agent_bindings.is_empty())
        {
            return Ok(None);
        }
        let stop = CancellationToken::new();
        let runtime = self.clone();
        let config = config.clone();
        let cancellation = stop.clone();
        let task = tokio::spawn(async move {
            runtime.run_agent_driver(config, cancellation).await;
        });
        Ok(Some(AgentServiceDriver { stop, task }))
    }
    async fn run_agent_driver(
        self: Arc<Self>,
        config: AgentServiceDriverConfig,
        stop: CancellationToken,
    ) {
        let mut ticks = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cursors = Cursors::new();
        let mut active = BTreeSet::new();
        let mut last_scope = None;
        let mut jobs = JoinSet::new();
        let mut job_ids = HashMap::new();
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                completion = jobs.join_next_with_id(), if !jobs.is_empty() => {
                    match completion {
                        Some(Ok((task_id, id))) => { job_ids.remove(&task_id); active.remove(&id); },
                        Some(Err(error)) => {
                            if let Some(id) = job_ids.remove(&error.id()) { active.remove(&id); }
                            tracing::warn!("agent driver task failed; durable evidence retained");
                        },
                        None => {},
                    }
                },
                _ = ticks.tick() => {
                    let available = config.max_parallel.saturating_sub(jobs.len());
                    if available == 0 { continue; }
                    if let Ok(work) = self.agent_work(&mut cursors, &mut last_scope, &active, available, config.scan_batch_size).await {
                        for (runtime, id) in work {
                            active.insert(id.clone());
                            let scheduled = id.clone();
                            let handle = jobs.spawn(async move {
                                let result = std::panic::AssertUnwindSafe(runtime.resume(id.2)).catch_unwind().await;
                                if !matches!(result, Ok(Ok(_))) {
                                    tracing::debug!(task_id=%id.2, "agent start or recovery refused; evidence retained");
                                }
                                id
                            });
                            job_ids.insert(handle.id(), scheduled);
                        }
                    } else {
                        tracing::warn!("agent acceptance scan unavailable; no work started");
                    }
                },
            }
        }
        while jobs.join_next().await.is_some() {}
    }

    async fn agent_work(
        &self,
        cursors: &mut Cursors,
        last_scope: &mut Option<(String, String)>,
        active: &BTreeSet<WorkId>,
        limit: usize,
        batch: usize,
    ) -> Result<Vec<Work>, String> {
        let mut out = Vec::new();
        let mut scopes: Vec<_> = self.scopes.iter().collect();
        if let Some(last) = last_scope.as_ref() {
            let start = scopes.partition_point(|(identity, _)| *identity <= last);
            let length = scopes.len();
            if length > 0 {
                scopes.rotate_left(start % length);
            }
        }
        for (identity, scope) in scopes {
            *last_scope = Some(identity.clone());
            if scope.agent_bindings.is_empty() {
                continue;
            }
            let mut entries = self
                .state
                .scan_keys(
                    &identity.0,
                    &identity.1,
                    KeyKind::Custom(ACCEPTANCE_KIND.into()),
                    None,
                )
                .await
                .map_err(|_| "agent acceptance scan unavailable")?;
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            if entries.is_empty() {
                cursors.remove(identity);
                continue;
            }
            let after = cursors.entry(identity.clone()).or_default();
            let start = entries.partition_point(|(key, _)| key <= after);
            let length = entries.len();
            entries.rotate_left(start % length);
            let bindings: BTreeMap<_, _> = scope
                .agent_bindings
                .values()
                .map(|runtime| (runtime.binding_digest(), runtime))
                .collect();
            for (key, raw) in entries.into_iter().take(batch) {
                after.clone_from(&key);
                // Only route a bounded hint. The fixed runtime checks the complete
                // acceptance, signature, lineage and original binding again.
                if raw.len() > 2 * 1024 * 1024 {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    continue;
                };
                let Some(runtime) = value
                    .get("binding_digest")
                    .and_then(|v| v.as_str())
                    .and_then(|digest| bindings.get(digest))
                else {
                    continue;
                };
                let Ok(id) = runtime.acceptance_task_id(&raw) else {
                    continue;
                };
                let expected = acteon_state::StateKey::new(
                    identity.0.as_str(),
                    identity.1.as_str(),
                    KeyKind::Custom(ACCEPTANCE_KIND.into()),
                    id.to_string(),
                )
                .canonical();
                let work_id = (identity.0.clone(), identity.1.clone(), id);
                if key != expected || active.contains(&work_id) {
                    continue;
                }
                let Ok(observation) = runtime.observe(id).await else {
                    continue;
                };
                if observation.future_starts_blocked {
                    continue;
                }
                if observation.execution.as_ref().is_some_and(|receipt| {
                    matches!(
                        receipt.status,
                        GovernedProviderStatus::Completed { .. }
                            | GovernedProviderStatus::InFlight { .. }
                            | GovernedProviderStatus::ReconciliationRequired { .. }
                    )
                }) {
                    continue;
                }
                if !(matches!(
                    observation.task.status.state,
                    acteon_core::TaskState::Submitted | acteon_core::TaskState::Working
                ) || (observation.continuation_pending
                    && observation.task.status.state == acteon_core::TaskState::InputRequired))
                {
                    continue;
                }
                out.push(((*runtime).clone(), work_id));
                if out.len() == limit {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }
}
