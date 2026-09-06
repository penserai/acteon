//! Rebuild queue discovery from authoritative records, then remove stale hints.

use acteon_state::{KeyKind, StateKey};
use tracing::warn;

use super::{
    Gateway, GatewayError, QUEUE_LEASED_KIND, QUEUE_PENDING_KIND, WORKER_TASK_KIND, pending_key,
};

impl Gateway {
    /// Idempotently publish a continuation whose identity is already stored in
    /// its receiving workflow. A lost create acknowledgement must not reset it.
    pub(crate) async fn ensure_worker_task(
        &self,
        expected: acteon_core::WorkerTask,
    ) -> Result<(), GatewayError> {
        if self
            .get_worker_task(&expected.namespace, &expected.tenant, &expected.task_id)
            .await?
            .is_none()
            && let Err(error) = self.enqueue_worker_task(expected.clone()).await
            && self
                .get_worker_task(&expected.namespace, &expected.tenant, &expected.task_id)
                .await?
                .is_none()
        {
            return Err(error);
        }
        let current = self
            .get_worker_task(&expected.namespace, &expected.tenant, &expected.task_id)
            .await?
            .ok_or_else(|| GatewayError::TaskQueue("continuation task disappeared".into()))?;
        if current.queue != expected.queue
            || current.action_type != expected.action_type
            || current.workflow_execution_id != expected.workflow_execution_id
            || current.chain_id != expected.chain_id
            || current.payload != expected.payload
        {
            return Err(GatewayError::TaskQueue(
                "continuation task identity mismatch".into(),
            ));
        }
        if current.status.is_active() {
            self.state
                .set(
                    &pending_key(
                        &current.namespace,
                        &current.tenant,
                        &current.queue,
                        &current.task_id,
                    ),
                    "active",
                    None,
                )
                .await?;
        }
        Ok(())
    }

    /// Reconcile queue indexes across scopes. Active records remain discoverable
    /// through leases and retries; terminal and orphaned hints are removed.
    /// Returns the number of active records successfully reindexed. Invalid or
    /// unreadable records are retained and logged. Independent records are still
    /// attempted when a write fails. Discovery/pruning errors are returned;
    /// terminal cleanup is best-effort and logs failures for the next sweep.
    ///
    /// Called on the background cleanup cadence when a gateway is attached.
    /// Embedders without that processor must call this after restart and
    /// periodically to repair interrupted initial enqueue writes.
    pub async fn reconcile_worker_task_indexes(&self) -> Result<usize, GatewayError> {
        let rows = self
            .state
            .scan_keys_by_kind(KeyKind::Custom(WORKER_TASK_KIND.into()))
            .await?;
        let mut indexed = 0;
        let mut first_error = None;
        for (key, raw) in rows {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[2] != WORKER_TASK_KIND {
                continue;
            }
            let task = match self.decode_worker_task(&raw, parts[0], parts[1], parts[3]) {
                Ok(task) => task,
                Err(error) => {
                    warn!(%error, task_id = parts[3], "invalid queue record; retained for repair");
                    continue;
                }
            };
            if task.status.is_active() {
                let key = pending_key(&task.namespace, &task.tenant, &task.queue, &task.task_id);
                match self.state.set(&key, "active", None).await {
                    Ok(()) => indexed += 1,
                    Err(error) => {
                        warn!(%error, task_id = %task.task_id, "queue discovery repair failed");
                        first_error.get_or_insert(GatewayError::State(error));
                    }
                }
            } else {
                self.remove_worker_indexes(&task).await;
            }
        }
        // Legacy leased hints are retained for active rows until settlement;
        // they never determine delivery or expiry in the new poll path.
        for kind in [QUEUE_PENDING_KIND, QUEUE_LEASED_KIND] {
            if let Err(error) = self.prune_worker_index(kind).await {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(indexed),
        }
    }

    async fn prune_worker_index(&self, kind: &str) -> Result<(), GatewayError> {
        let rows = self
            .state
            .scan_keys_by_kind(KeyKind::Custom(kind.into()))
            .await?;
        let mut first_error = None;
        for (canonical, _) in rows {
            let parts: Vec<_> = canonical.splitn(5, ':').collect();
            if parts.len() != 5 || parts[2] != kind {
                continue;
            }
            let task = match self.get_worker_task(parts[0], parts[1], parts[4]).await {
                Ok(task) => task,
                Err(error) => {
                    warn!(%error, task_id = parts[4], "queue index cannot be validated; retained for repair");
                    if matches!(error, GatewayError::State(_)) {
                        first_error.get_or_insert(error);
                    }
                    continue;
                }
            };
            if task.is_some_and(|task| task.status.is_active() && task.queue == parts[3]) {
                continue;
            }
            let key = StateKey::new(
                parts[0],
                parts[1],
                KeyKind::Custom(kind.into()),
                format!("{}:{}", parts[3], parts[4]),
            );
            if let Err(error) = self.state.delete(&key).await {
                warn!(%error, "stale queue index cleanup failed");
                first_error.get_or_insert(GatewayError::State(error));
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
