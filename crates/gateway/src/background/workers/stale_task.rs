use std::sync::Arc;

use chrono::Utc;
use tracing::{info, warn};

use acteon_core::Task;
use acteon_state::KeyKind;

use super::super::BackgroundProcessor;
use crate::task_engine::{TaskEngine, TaskScope};

impl BackgroundProcessor {
    /// Run the stale-task reaper.
    ///
    /// Scans every A2A task row and transitions each **stale** task —
    /// one sitting in a non-terminal state past its `working_ttl_ms`
    /// without recorded progress — to `Failed`. A zombie task that no
    /// producer is advancing would otherwise stay non-terminal
    /// forever; `Task::is_stale_at` already reports it as stale on
    /// read, and this worker makes that verdict durable.
    ///
    /// Each candidate is re-checked for staleness inside the task
    /// engine's compare-and-swap (see `TaskEngine::fail_if_stale`), so
    /// a task that recorded progress between the scan and the write is
    /// left untouched.
    pub(crate) async fn run_stale_task_reaper(
        &self,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = Utc::now();
        let mut engine = TaskEngine::new(self.state.clone());
        if let Some(audit) = &self.audit {
            engine = engine.with_audit(Arc::clone(audit));
        }
        let entries = self.state.scan_keys_by_kind(KeyKind::A2aTask).await?;

        let mut reaped = 0u64;
        let mut errors = 0u64;

        for (key, raw_value) in entries {
            // Task rows are stored as plain JSON — the TaskEngine does
            // not encrypt them — so no decryption step is needed.
            let task: Task = match serde_json::from_str(&raw_value) {
                Ok(t) => t,
                Err(e) => {
                    warn!(key = %key, error = %e, "stale-task reaper: skipping malformed task row");
                    continue;
                }
            };
            // Cheap pre-filter on the scanned snapshot; the engine
            // re-checks against the fresh row before it commits.
            if !task.is_stale_at(now) {
                continue;
            }
            let scope = TaskScope::new(&task.namespace, &task.tenant);
            match engine.fail_if_stale(&scope, &task.id, now).await {
                Ok(Some(t)) => {
                    reaped += 1;
                    self.metrics.increment_stale_tasks_reaped();
                    info!(
                        namespace = %t.namespace,
                        tenant = %t.tenant,
                        task_id = %t.id,
                        "stale-task reaper: task failed (exceeded working TTL)"
                    );
                }
                Ok(None) => {
                    // Recorded progress or reached a terminal state
                    // between the scan and the commit — leave it.
                }
                Err(e) => {
                    warn!(
                        namespace = %task.namespace,
                        tenant = %task.tenant,
                        task_id = %task.id,
                        error = %e,
                        "stale-task reaper: error failing stale task"
                    );
                    errors += 1;
                    self.metrics.increment_stale_task_reaper_errors();
                }
            }
        }

        if reaped > 0 || errors > 0 {
            info!(reaped, errors, "stale-task reaper cycle complete");
        }

        Ok(())
    }
}
