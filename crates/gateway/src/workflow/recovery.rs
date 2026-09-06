//! Repair secondary discovery from receiving workflow records.
use super::{
    COMPLETED_WORKFLOW_TTL, FollowUp, Gateway, GatewayError, KeyKind, WORKFLOW_EXEC_KIND,
    WorkflowAwait, WorkflowExecution, WorkflowStatus, timer_key,
};
impl Gateway {
    pub(super) fn terminal_workflow_follow_ups(exec: &WorkflowExecution) -> Vec<FollowUp> {
        if !exec.close_pending {
            return Vec::new();
        }
        let payload = match exec.status {
            WorkflowStatus::Completed => {
                serde_json::json!({"status":"completed", "result":exec.result})
            }
            WorkflowStatus::Failed => serde_json::json!({"status":"failed", "error":exec.error}),
            WorkflowStatus::Cancelled => serde_json::json!({"status":"cancelled"}),
            _ => return Vec::new(),
        };
        Self::close_follow_ups(exec, payload)
    }

    pub(super) async fn repair_workflow_discovery(
        &self,
        exec: &WorkflowExecution,
    ) -> Result<(), GatewayError> {
        if !exec.status.is_active() {
            return Ok(());
        }
        if let Some(id) = &exec.current_task_id {
            let mut task = self.build_continuation_task(exec);
            task.task_id.clone_from(id);
            self.ensure_worker_task(task).await?;
        }
        let deadline = match &exec.awaiting {
            Some(WorkflowAwait::Timer { fire_at, .. }) => Some(*fire_at),
            Some(WorkflowAwait::Signal { timeout_at, .. }) => *timeout_at,
            None => None,
        };
        if let Some(at) = deadline {
            self.state
                .index_timeout(
                    &timer_key(&exec.namespace, &exec.tenant, &exec.execution_id),
                    at.timestamp_millis(),
                )
                .await?;
        }
        Ok(())
    }

    /// Rebuild continuation and timer discovery after interrupted workflow
    /// writes. Each receiving execution is reloaded under its workflow lock.
    pub async fn reconcile_workflow_discovery(&self) -> Result<usize, GatewayError> {
        let rows = self
            .state
            .scan_keys_by_kind(KeyKind::Custom(WORKFLOW_EXEC_KIND.into()))
            .await?;
        let mut repaired = 0;
        let mut first_error = None;
        for (key, _) in rows {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[2] != WORKFLOW_EXEC_KIND {
                continue;
            }
            let repair = async {
                let guard = self.lock_workflow(parts[3]).await?;
                let result = async {
                    if let Some(exec) = self.load_workflow(parts[0], parts[1], parts[3]).await? {
                        self.repair_workflow_discovery(&exec).await?;
                        let version = exec.state_version.filter(|_| !exec.status.is_active());
                        return Ok::<_, GatewayError>((
                            Self::terminal_workflow_follow_ups(&exec),
                            version,
                        ));
                    }
                    Ok((Vec::new(), None))
                }
                .await;
                guard
                    .release()
                    .await
                    .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
                let (follow_ups, version) = result?;
                self.run_follow_ups(parts[0], parts[1], follow_ups).await?;
                self.finish_workflow_close(parts[0], parts[1], parts[3], version)
                    .await
            }
            .await;
            match repair {
                Ok(()) => repaired += 1,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(repaired),
        }
    }
    pub(super) async fn finish_workflow_close(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        delivered_version: Option<u64>,
    ) -> Result<(), GatewayError> {
        let Some(version) = delivered_version else {
            return Ok(());
        };
        let guard = self.lock_workflow(id).await?;
        let result = async {
            if let Some(mut exec) = self.load_workflow(namespace, tenant, id).await?
                && !exec.status.is_active()
                && exec.close_pending
                // Only acknowledge the terminal snapshot whose effects ran.
                && exec.state_version == Some(version)
            {
                exec.close_pending = false;
                self.persist_workflow(&mut exec, Some(COMPLETED_WORKFLOW_TTL))
                    .await?;
            }
            Ok(())
        }
        .await;
        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }
}
