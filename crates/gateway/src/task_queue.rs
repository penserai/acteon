//! Worker task queues: the external-worker execution substrate.
//!
//! Tasks are persisted in the state store and indexed per queue. Workers
//! lease tasks via [`Gateway::poll_worker_tasks`], extend leases with
//! [`Gateway::heartbeat_worker_task`], and settle them with
//! [`Gateway::complete_worker_task`] / [`Gateway::fail_worker_task`]. All
//! transitions go through compare-and-swap, so concurrent workers polling
//! the same queue never double-lease a task. Expired leases are reclaimed
//! lazily on poll. Active discovery survives lease/retry transitions; cleanup
//! reconciliation repairs interrupted initial index writes. Index rows are hints,
//! while the scoped task record controls ownership and delivery eligibility.
//!
//! Tasks enqueued by a `worker` chain step resume their owning chain on
//! completion; tasks that drive workflow executions are routed to the
//! workflow engine.

mod recovery;

use std::time::Duration;

use chrono::Utc;
use tracing::{debug, warn};

use acteon_core::chain::WaitState;
use acteon_core::{
    Action, ChainStatus, DEFAULT_TASK_LEASE_SECONDS, ExecutionEventType, MAX_TASK_LEASE_SECONDS,
    StepResult, WorkerTask, WorkerTaskStatus,
};
use acteon_state::{CasResult, KeyKind, StateKey};

use crate::error::GatewayError;
use crate::gateway::Gateway;

/// State-store kind for worker task records.
const WORKER_TASK_KIND: &str = "worker_task";
/// Per-queue active discovery (`{queue}:{task_id}`), retaining its legacy kind name.
const QUEUE_PENDING_KIND: &str = "queue_pending";
/// Legacy leased index, read only for migration cleanup.
const QUEUE_LEASED_KIND: &str = "queue_leased";

/// CAS retry budget for task transitions.
const MAX_CAS_ATTEMPTS: usize = 5;
/// TTL for terminal task records.
const COMPLETED_TASK_TTL: Duration = Duration::from_secs(24 * 3600);

fn task_key(namespace: &str, tenant: &str, task_id: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(WORKER_TASK_KIND.into()),
        task_id,
    )
}

fn pending_key(namespace: &str, tenant: &str, queue: &str, task_id: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(QUEUE_PENDING_KIND.into()),
        format!("{queue}:{task_id}"),
    )
}

fn leased_key(namespace: &str, tenant: &str, queue: &str, task_id: &str) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(QUEUE_LEASED_KIND.into()),
        format!("{queue}:{task_id}"),
    )
}

/// Retry backoff before a failed/reclaimed task becomes leasable again:
/// `2^attempt` seconds, capped at 60.
fn retry_backoff(attempt: u32) -> chrono::Duration {
    chrono::Duration::seconds(2_i64.saturating_pow(attempt.min(6)).min(60))
}

/// Validate a queue name. `:` is the canonical-key delimiter, so it must be
/// excluded or queues `etl` and `etl:high` would cross-contaminate the
/// per-queue prefix scans; restrict to a safe charset outright.
pub(crate) fn validate_queue_name(queue: &str) -> Result<(), GatewayError> {
    if queue.is_empty() {
        return Err(GatewayError::TaskQueue(
            "queue name must not be empty".into(),
        ));
    }
    if !queue
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(GatewayError::TaskQueue(format!(
            "invalid queue name `{queue}`: only [A-Za-z0-9._-] is allowed"
        )));
    }
    Ok(())
}

fn validate_worker_scope(
    task: &WorkerTask,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<(), GatewayError> {
    validate_queue_name(&task.queue)?;
    if namespace.is_empty()
        || tenant.is_empty()
        || id.is_empty()
        || namespace.contains(':')
        || tenant.contains(':')
        || task.namespace != namespace
        || task.tenant != tenant
        || task.task_id != id
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        || task.max_attempts == 0
    {
        return Err(GatewayError::TaskQueue(
            "invalid worker task scope or attempt budget".into(),
        ));
    }
    Ok(())
}

impl Gateway {
    /// Enqueue a new task. Existing IDs are never overwritten, including when
    /// an earlier call persisted its task but lost the index-write response.
    pub async fn enqueue_worker_task(&self, task: WorkerTask) -> Result<WorkerTask, GatewayError> {
        validate_worker_scope(&task, &task.namespace, &task.tenant, &task.task_id)?;
        if task.status != WorkerTaskStatus::Pending
            || task.attempt != 0
            || task.lease_token.is_some()
            || task.lease_expires_at.is_some()
            || task.worker_id.is_some()
            || task.result.is_some()
            || task.error.is_some()
        {
            return Err(GatewayError::TaskQueue(
                "enqueue requires a new pending task".into(),
            ));
        }
        let key = task_key(&task.namespace, &task.tenant, &task.task_id);
        let json = self.encode_worker_task(&task)?;
        if !self.state.check_and_set(&key, &json, None).await? {
            return Err(GatewayError::TaskQueue(format!(
                "task already exists: {}",
                task.task_id
            )));
        }
        self.state
            .set(
                &pending_key(&task.namespace, &task.tenant, &task.queue, &task.task_id),
                "active",
                None,
            )
            .await?;
        debug!(task_id = %task.task_id, queue = %task.queue, "worker task enqueued");
        Ok(task)
    }

    fn decode_worker_task(
        &self,
        raw: &str,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<WorkerTask, GatewayError> {
        let clear = self.decrypt_state_value(raw)?;
        let task: WorkerTask = serde_json::from_str(&clear)
            .map_err(|e| GatewayError::TaskQueue(format!("failed to deserialize task: {e}")))?;
        validate_worker_scope(&task, namespace, tenant, id)?;
        Ok(task)
    }

    fn encode_worker_task(&self, task: &WorkerTask) -> Result<String, GatewayError> {
        let clear = serde_json::to_string(task)
            .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
        self.encrypt_state_value(&clear)
    }

    async fn remove_worker_indexes(&self, task: &WorkerTask) {
        for key in [
            pending_key(&task.namespace, &task.tenant, &task.queue, &task.task_id),
            leased_key(&task.namespace, &task.tenant, &task.queue, &task.task_id),
        ] {
            if let Err(error) = self.state.delete(&key).await {
                warn!(%error, task_id = %task.task_id, "terminal queue index cleanup will retry");
            }
        }
    }

    /// Load a worker task by ID.
    pub async fn get_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Option<WorkerTask>, GatewayError> {
        match self
            .state
            .get(&task_key(namespace, tenant, task_id))
            .await?
        {
            Some(raw) => self
                .decode_worker_task(&raw, namespace, tenant, task_id)
                .map(Some),
            None => Ok(None),
        }
    }

    /// List tasks on a queue, optionally filtered by status. Scans the task
    /// records for the namespace/tenant (visibility helper, not a hot path).
    pub async fn list_worker_tasks(
        &self,
        namespace: &str,
        tenant: &str,
        queue: Option<&str>,
        status: Option<WorkerTaskStatus>,
    ) -> Result<Vec<WorkerTask>, GatewayError> {
        let entries = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(WORKER_TASK_KIND.into()),
                None,
            )
            .await?;
        let mut tasks = Vec::new();
        let prefix = format!("{namespace}:{tenant}:{WORKER_TASK_KIND}:");
        for (key, raw) in entries {
            let Some(id) = key.strip_prefix(&prefix) else {
                continue;
            };
            let Ok(task) = self.decode_worker_task(&raw, namespace, tenant, id) else {
                continue;
            };
            if queue.is_some_and(|q| task.queue != q) {
                continue;
            }
            if status.is_some_and(|s| task.status != s) {
                continue;
            }
            tasks.push(task);
        }
        tasks.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        Ok(tasks)
    }

    /// Lease up to `max_tasks` pending tasks from a queue for a worker.
    ///
    /// Reclaims expired leases while visiting active records, even after the
    /// delivery limit is reached. Reclaimed tasks retain their retry backoff.
    pub async fn poll_worker_tasks(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
        max_tasks: usize,
        lease_seconds: Option<u64>,
        worker_id: Option<&str>,
    ) -> Result<Vec<WorkerTask>, GatewayError> {
        validate_queue_name(queue)?;
        let lease_seconds = lease_seconds
            .unwrap_or(DEFAULT_TASK_LEASE_SECONDS)
            .clamp(1, MAX_TASK_LEASE_SECONDS);

        let pending = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(QUEUE_PENDING_KIND.into()),
                Some(&format!("{queue}:")),
            )
            .await?;

        let mut leased = Vec::new();
        let prefix = format!("{namespace}:{tenant}:{QUEUE_PENDING_KIND}:{queue}:");
        for (index_key, _) in pending {
            let Some(task_id) = index_key.strip_prefix(&prefix) else {
                continue;
            };
            let key = task_key(namespace, tenant, task_id);
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                let _ = self
                    .state
                    .delete(&pending_key(namespace, tenant, queue, task_id))
                    .await;
                continue;
            };
            let task = match self.decode_worker_task(&raw, namespace, tenant, task_id) {
                Ok(task) => task,
                Err(error) => {
                    warn!(%error, task_id, "invalid queue record; retained for repair");
                    continue;
                }
            };
            if task.queue != queue {
                let _ = self
                    .state
                    .delete(&pending_key(namespace, tenant, queue, task_id))
                    .await;
                continue;
            }
            if !task.status.is_active() {
                self.remove_worker_indexes(&task).await;
                continue;
            }
            let now = self.clock.now();
            if task.status == WorkerTaskStatus::Leased {
                if task.lease_expires_at.is_none_or(|at| now >= at) {
                    self.reclaim_worker_lease(task, version, now).await?;
                }
                continue;
            }
            if leased.len() >= max_tasks.max(1)
                || !task.leasable(now)
                || task.attempt >= task.max_attempts
            {
                continue;
            }
            let mut task = task;

            task.status = WorkerTaskStatus::Leased;
            task.attempt += 1;
            task.lease_token = Some(uuid::Uuid::new_v4().to_string());
            #[allow(clippy::cast_possible_wrap)]
            let expires = now + chrono::Duration::seconds(lease_seconds as i64);
            task.lease_expires_at = Some(expires);
            task.worker_id = worker_id.map(ToOwned::to_owned);
            task.not_before = None;
            task.updated_at = now;

            let json = self.encode_worker_task(&task)?;
            if self
                .state
                .compare_and_swap(&key, version, &json, None)
                .await?
                == CasResult::Ok
            {
                // Keep discovery for the entire active lifetime. A delayed
                // poll must never delete a later owner's retry entry.
                leased.push(task);
            }
        }
        Ok(leased)
    }

    /// Extend the lease on a task. The caller must present the lease token
    /// returned by poll.
    pub async fn heartbeat_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        extend_seconds: Option<u64>,
    ) -> Result<WorkerTask, GatewayError> {
        let extend = extend_seconds
            .unwrap_or(DEFAULT_TASK_LEASE_SECONDS)
            .clamp(1, MAX_TASK_LEASE_SECONDS);
        let key = task_key(namespace, tenant, task_id);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(GatewayError::TaskQueue(format!(
                    "task not found: {task_id}"
                )));
            };
            let mut task = self.decode_worker_task(&raw, namespace, tenant, task_id)?;
            let now = self.clock.now();
            Self::verify_lease(&task, lease_token, now)?;
            #[allow(clippy::cast_possible_wrap)]
            let expires = now + chrono::Duration::seconds(extend as i64);
            task.lease_expires_at = Some(expires);
            task.updated_at = now;
            let json = self.encode_worker_task(&task)?;
            match self
                .state
                .compare_and_swap(&key, version, &json, None)
                .await?
            {
                CasResult::Ok => {
                    return Ok(task);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(GatewayError::TaskQueue(format!(
            "heartbeat contention exhausted for task {task_id}"
        )))
    }

    /// Complete a leased task with a result. Resumes the owning chain or
    /// workflow execution, if any.
    pub async fn complete_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        result: serde_json::Value,
    ) -> Result<WorkerTask, GatewayError> {
        let task = self
            .settle_worker_task(namespace, tenant, task_id, lease_token, |task, now| {
                task.status = WorkerTaskStatus::Completed;
                task.result = Some(result.clone());
                task.lease_expires_at = None;
                task.updated_at = now;
            })
            .await?;

        if task.chain_id.is_some() {
            self.resume_chain_worker_step(&task, Ok(task.result.clone().unwrap_or_default()))
                .await;
        }
        self.route_workflow_task_result(&task).await;
        Ok(task)
    }

    /// Fail a leased task. Retryable failures within the attempt budget are
    /// re-queued with backoff; otherwise the task fails terminally, goes to
    /// the DLQ, and fails the owning chain step (per its failure policy).
    pub async fn fail_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        error: &str,
        retryable: bool,
    ) -> Result<WorkerTask, GatewayError> {
        let task = self
            .settle_worker_task(namespace, tenant, task_id, lease_token, |task, now| {
                if retryable && task.attempt < task.max_attempts {
                    task.status = WorkerTaskStatus::Pending;
                    task.not_before = Some(now + retry_backoff(task.attempt));
                    task.error = Some(error.to_owned());
                    task.lease_token = None;
                    task.lease_expires_at = None;
                    task.worker_id = None;
                } else {
                    task.status = WorkerTaskStatus::Failed;
                    task.error = Some(error.to_owned());
                    task.lease_expires_at = None;
                }
                task.updated_at = now;
            })
            .await?;

        if task.status == WorkerTaskStatus::Pending {
            debug!(
                task_id = %task_id,
                attempt = task.attempt,
                max_attempts = task.max_attempts,
                "worker task failed; re-queued with backoff"
            );
        } else {
            self.push_task_to_dlq(&task).await;
            if task.chain_id.is_some() {
                self.resume_chain_worker_step(&task, Err(error.to_owned()))
                    .await;
            }
            self.route_workflow_task_result(&task).await;
        }
        Ok(task)
    }

    /// Cancel a task that has not completed. Best-effort companion for
    /// worker-step timeouts and operator intervention.
    pub async fn cancel_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<WorkerTask, GatewayError> {
        let key = task_key(namespace, tenant, task_id);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(GatewayError::TaskQueue(format!(
                    "task not found: {task_id}"
                )));
            };
            let mut task = self.decode_worker_task(&raw, namespace, tenant, task_id)?;
            if !task.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "task is not active (status: {:?})",
                    task.status
                )));
            }
            let now = self.clock.now();
            task.status = WorkerTaskStatus::Cancelled;
            task.lease_expires_at = None;
            task.updated_at = now;
            let json = self.encode_worker_task(&task)?;
            match self
                .state
                .compare_and_swap(&key, version, &json, Some(COMPLETED_TASK_TTL))
                .await?
            {
                CasResult::Ok => {
                    self.remove_worker_indexes(&task).await;
                    return Ok(task);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(GatewayError::TaskQueue(format!(
            "cancel contention exhausted for task {task_id}"
        )))
    }

    /// Shared CAS loop for terminal/requeue transitions that require a valid
    /// lease token.
    async fn settle_worker_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        mutate: impl Fn(&mut WorkerTask, chrono::DateTime<Utc>),
    ) -> Result<WorkerTask, GatewayError> {
        let key = task_key(namespace, tenant, task_id);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Err(GatewayError::TaskQueue(format!(
                    "task not found: {task_id}"
                )));
            };
            let mut task = self.decode_worker_task(&raw, namespace, tenant, task_id)?;
            let now = self.clock.now();
            Self::verify_lease(&task, lease_token, now)?;

            mutate(&mut task, now);
            let terminal = !task.status.is_active();
            let ttl = terminal.then_some(COMPLETED_TASK_TTL);
            let json = self.encode_worker_task(&task)?;
            match self
                .state
                .compare_and_swap(&key, version, &json, ttl)
                .await?
            {
                CasResult::Ok => {
                    if terminal {
                        self.remove_worker_indexes(&task).await;
                    }
                    return Ok(task);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(GatewayError::TaskQueue(format!(
            "settle contention exhausted for task {task_id}"
        )))
    }

    fn verify_lease(
        task: &WorkerTask,
        lease_token: &str,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        if task.status != WorkerTaskStatus::Leased {
            return Err(GatewayError::TaskQueue(format!(
                "task {} is not leased (status: {:?})",
                task.task_id, task.status
            )));
        }
        if task.lease_expires_at.is_none_or(|expires| now >= expires) {
            return Err(GatewayError::TaskQueue(format!(
                "lease expired for task {}",
                task.task_id
            )));
        }
        if task.lease_token.as_deref() != Some(lease_token) {
            return Err(GatewayError::TaskQueue(format!(
                "lease token mismatch for task {} (lease may have expired and been re-delivered)",
                task.task_id
            )));
        }
        Ok(())
    }

    /// Reclaim from the same versioned row used for the expiry decision.
    async fn reclaim_worker_lease(
        &self,
        mut task: WorkerTask,
        version: u64,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), GatewayError> {
        let exhausted = task.attempt >= task.max_attempts;
        if exhausted {
            task.status = WorkerTaskStatus::Failed;
            task.error = Some(format!(
                "lease expired after attempt {}/{} (worker did not heartbeat)",
                task.attempt, task.max_attempts
            ));
        } else {
            task.status = WorkerTaskStatus::Pending;
            task.not_before = Some(now + retry_backoff(task.attempt));
        }
        task.lease_token = None;
        task.lease_expires_at = None;
        task.worker_id = None;
        task.updated_at = now;
        let json = self.encode_worker_task(&task)?;
        let key = task_key(&task.namespace, &task.tenant, &task.task_id);
        if self
            .state
            .compare_and_swap(
                &key,
                version,
                &json,
                exhausted.then_some(COMPLETED_TASK_TTL),
            )
            .await?
            == CasResult::Ok
            && exhausted
        {
            self.remove_worker_indexes(&task).await;
            self.push_task_to_dlq(&task).await;
            if task.chain_id.is_some() {
                self.resume_chain_worker_step(&task, Err(task.error.clone().unwrap_or_default()))
                    .await;
            }
            self.route_workflow_task_result(&task).await;
        }
        Ok(())
    }

    /// Push a terminally-failed task to the dead letter queue (best-effort).
    async fn push_task_to_dlq(&self, task: &WorkerTask) {
        if let Some(ref dlq) = self.dlq {
            let action = Action::new(
                task.namespace.as_str(),
                task.tenant.as_str(),
                format!("queue:{}", task.queue),
                &task.action_type,
                task.payload.clone(),
            );
            if dlq
                .push(
                    action,
                    task.error
                        .clone()
                        .unwrap_or_else(|| "worker task failed".into()),
                    task.attempt,
                )
                .await
                .is_err()
            {
                tracing::error!("failed action was not retained in dead-letter storage");
            }
        }
    }

    /// Route a settled task to the workflow engine when it drives a
    /// workflow execution. No-op for plain queue tasks.
    pub(crate) async fn route_workflow_task_result(&self, task: &WorkerTask) {
        if task.workflow_execution_id.is_some() {
            self.settle_workflow_task(task).await;
        }
    }

    /// Resume the chain that owns a worker-step task with the task's
    /// terminal outcome. Best-effort: the chain may have been cancelled or
    /// timed out while the worker ran.
    pub(crate) async fn resume_chain_worker_step(
        &self,
        task: &WorkerTask,
        outcome: Result<serde_json::Value, String>,
    ) {
        let (Some(chain_id), Some(step_idx)) = (task.chain_id.as_deref(), task.step_index) else {
            return;
        };
        if let Err(e) = self
            .resume_chain_worker_step_inner(task, chain_id, step_idx, outcome)
            .await
        {
            warn!(
                chain_id = %chain_id,
                task_id = %task.task_id,
                error = %e,
                "failed to resume chain from worker task result"
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn resume_chain_worker_step_inner(
        &self,
        task: &WorkerTask,
        chain_id: &str,
        step_idx: usize,
        outcome: Result<serde_json::Value, String>,
    ) -> Result<(), GatewayError> {
        let namespace = task.namespace.as_str();
        let tenant = task.tenant.as_str();

        let lock_name = format!("chain:{chain_id}");
        let guard = self
            .lock
            .acquire(&lock_name, Duration::from_secs(60), Duration::from_secs(5))
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;

        let result: Result<(), GatewayError> = async {
            let Some(mut chain_state) = self.get_chain_status(namespace, tenant, chain_id).await?
            else {
                return Ok(());
            };

            // Only resume when the chain is still waiting on this exact task.
            let waiting_on_this_task = matches!(
                &chain_state.wait_state,
                Some(WaitState::Worker { task_id, step_index, .. })
                    if task_id == &task.task_id && *step_index == step_idx
            );
            if !waiting_on_this_task || chain_state.status != ChainStatus::WaitingWorker {
                debug!(
                    chain_id = %chain_id,
                    task_id = %task.task_id,
                    "chain no longer waiting on this task; skipping resume"
                );
                return Ok(());
            }

            let chain_config = self.execution_config(&chain_state).await?.ok_or_else(|| {
                GatewayError::ChainError(format!(
                    "chain configuration not found: {}",
                    chain_state.chain_name
                ))
            })?;
            if step_idx >= chain_config.steps.len() {
                return Ok(());
            }
            let step_config = chain_config.steps[step_idx].clone();
            let step_index_map = chain_config.step_index_map();

            chain_state.wait_state = None;
            chain_state.status = ChainStatus::Running;

            let chain_key = StateKey::new(namespace, tenant, KeyKind::Chain, chain_id);
            let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingChains, chain_id);
            let now = self.clock.now();

            match outcome {
                Ok(result_value) => {
                    self.append_execution_history(
                        namespace,
                        tenant,
                        chain_id,
                        ExecutionEventType::TaskCompleted {
                            step_name: step_config.name.clone(),
                            task_id: task.task_id.clone(),
                            attempt: task.attempt,
                        },
                        None,
                    )
                    .await;
                    let step_result = StepResult::new(
                        step_config.name.clone(),
                        true,
                        Some(result_value),
                        None,
                        now,
                    );
                    self.complete_wait_step(
                        namespace,
                        tenant,
                        chain_id,
                        &chain_key,
                        &pending_key,
                        &chain_config,
                        &mut chain_state,
                        step_idx,
                        &step_config,
                        step_result,
                        &step_index_map,
                        "chain_step_completed",
                    )
                    .await
                }
                Err(error) => {
                    self.append_execution_history(
                        namespace,
                        tenant,
                        chain_id,
                        ExecutionEventType::TaskFailed {
                            step_name: step_config.name.clone(),
                            task_id: task.task_id.clone(),
                            attempt: task.attempt,
                            error: error.clone(),
                        },
                        None,
                    )
                    .await;
                    let step_result =
                        StepResult::new(step_config.name.clone(), false, None, Some(error), now);
                    self.fail_wait_step(
                        namespace,
                        tenant,
                        chain_id,
                        &chain_key,
                        &pending_key,
                        &chain_config,
                        &mut chain_state,
                        step_idx,
                        &step_config,
                        step_result,
                        &step_index_map,
                    )
                    .await
                }
            }
        }
        .await;

        guard
            .release()
            .await
            .map_err(|e| GatewayError::LockFailed(e.to_string()))?;
        result
    }
}
