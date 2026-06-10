//! Worker task queues: the external-worker execution substrate.
//!
//! Tasks are persisted in the state store and indexed per queue. Workers
//! lease tasks via [`Gateway::poll_worker_tasks`], extend leases with
//! [`Gateway::heartbeat_worker_task`], and settle them with
//! [`Gateway::complete_worker_task`] / [`Gateway::fail_worker_task`]. All
//! transitions go through compare-and-swap, so concurrent workers polling
//! the same queue never double-lease a task. Expired leases are reclaimed
//! lazily on poll (workers poll continuously, so reclamation latency is
//! bounded by the poll interval).
//!
//! Tasks enqueued by a `worker` chain step resume their owning chain on
//! completion; tasks that drive workflow executions are routed to the
//! workflow engine.

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
/// State-store kind for the per-queue pending index (`{queue}:{task_id}`).
const QUEUE_PENDING_KIND: &str = "queue_pending";
/// State-store kind for the per-queue leased index (`{queue}:{task_id}`).
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

impl Gateway {
    /// Enqueue a task on a named worker queue.
    pub async fn enqueue_worker_task(&self, task: WorkerTask) -> Result<WorkerTask, GatewayError> {
        if task.queue.is_empty() {
            return Err(GatewayError::TaskQueue(
                "queue name must not be empty".into(),
            ));
        }
        let key = task_key(&task.namespace, &task.tenant, &task.task_id);
        let json = serde_json::to_string(&task)
            .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
        self.state.set(&key, &json, None).await?;
        self.state
            .set(
                &pending_key(&task.namespace, &task.tenant, &task.queue, &task.task_id),
                "pending",
                None,
            )
            .await?;
        debug!(
            task_id = %task.task_id,
            queue = %task.queue,
            action_type = %task.action_type,
            "worker task enqueued"
        );
        Ok(task)
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
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to deserialize task: {e}"))),
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
        for (_, raw) in entries {
            let Ok(task) = serde_json::from_str::<WorkerTask>(&raw) else {
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
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(tasks)
    }

    /// Lease up to `max_tasks` pending tasks from a queue for a worker.
    ///
    /// Also reclaims expired leases on the queue first, so abandoned tasks
    /// are re-delivered (or failed once their attempt budget is exhausted).
    pub async fn poll_worker_tasks(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
        max_tasks: usize,
        lease_seconds: Option<u64>,
        worker_id: Option<&str>,
    ) -> Result<Vec<WorkerTask>, GatewayError> {
        let lease_seconds = lease_seconds
            .unwrap_or(DEFAULT_TASK_LEASE_SECONDS)
            .clamp(1, MAX_TASK_LEASE_SECONDS);

        self.reclaim_expired_leases(namespace, tenant, queue)
            .await?;

        let pending = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(QUEUE_PENDING_KIND.into()),
                Some(&format!("{queue}:")),
            )
            .await?;

        let now = Utc::now();
        let mut leased = Vec::new();
        for (index_key, _) in pending {
            if leased.len() >= max_tasks.max(1) {
                break;
            }
            // Index keys are canonical `{ns}:{tenant}:{kind}:{queue}:{task_id}`;
            // the task ID is the last segment.
            let Some(task_id) = index_key.rsplit(':').next().map(str::to_owned) else {
                continue;
            };
            let key = task_key(namespace, tenant, &task_id);
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                // Stale index entry; clean it up.
                let _ = self
                    .state
                    .delete(&pending_key(namespace, tenant, queue, &task_id))
                    .await;
                continue;
            };
            let Ok(mut task) = serde_json::from_str::<WorkerTask>(&raw) else {
                continue;
            };
            if !task.leasable(now) {
                continue;
            }

            task.status = WorkerTaskStatus::Leased;
            task.attempt += 1;
            task.lease_token = Some(uuid::Uuid::new_v4().to_string());
            #[allow(clippy::cast_possible_wrap)]
            let expires = now + chrono::Duration::seconds(lease_seconds as i64);
            task.lease_expires_at = Some(expires);
            task.worker_id = worker_id.map(ToOwned::to_owned);
            task.not_before = None;
            task.updated_at = now;

            let json = serde_json::to_string(&task)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
            match self
                .state
                .compare_and_swap(&key, version, &json, None)
                .await?
            {
                CasResult::Ok => {
                    let _ = self
                        .state
                        .delete(&pending_key(namespace, tenant, queue, &task_id))
                        .await;
                    self.state
                        .set(
                            &leased_key(namespace, tenant, queue, &task_id),
                            &expires.timestamp_millis().to_string(),
                            None,
                        )
                        .await?;
                    leased.push(task);
                }
                // Another worker won the race; move on.
                CasResult::Conflict { .. } => {}
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
            let mut task: WorkerTask = serde_json::from_str(&raw)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to deserialize task: {e}")))?;
            Self::verify_lease(&task, lease_token)?;

            let now = Utc::now();
            #[allow(clippy::cast_possible_wrap)]
            let expires = now + chrono::Duration::seconds(extend as i64);
            task.lease_expires_at = Some(expires);
            task.updated_at = now;
            let json = serde_json::to_string(&task)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
            match self
                .state
                .compare_and_swap(&key, version, &json, None)
                .await?
            {
                CasResult::Ok => {
                    self.state
                        .set(
                            &leased_key(namespace, tenant, &task.queue, task_id),
                            &expires.timestamp_millis().to_string(),
                            None,
                        )
                        .await?;
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
        // Decide retry vs terminal from the current attempt count.
        let current = self
            .get_worker_task(namespace, tenant, task_id)
            .await?
            .ok_or_else(|| GatewayError::TaskQueue(format!("task not found: {task_id}")))?;
        let will_retry = retryable && current.attempt < current.max_attempts;

        let task = self
            .settle_worker_task(namespace, tenant, task_id, lease_token, |task, now| {
                if will_retry {
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

        if will_retry {
            self.state
                .set(
                    &pending_key(namespace, tenant, &task.queue, task_id),
                    "pending",
                    None,
                )
                .await?;
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
            let mut task: WorkerTask = serde_json::from_str(&raw)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to deserialize task: {e}")))?;
            if !task.status.is_active() {
                return Err(GatewayError::TaskQueue(format!(
                    "task is not active (status: {:?})",
                    task.status
                )));
            }
            let now = Utc::now();
            task.status = WorkerTaskStatus::Cancelled;
            task.lease_expires_at = None;
            task.updated_at = now;
            let json = serde_json::to_string(&task)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
            match self
                .state
                .compare_and_swap(&key, version, &json, Some(COMPLETED_TASK_TTL))
                .await?
            {
                CasResult::Ok => {
                    let _ = self
                        .state
                        .delete(&pending_key(namespace, tenant, &task.queue, task_id))
                        .await;
                    let _ = self
                        .state
                        .delete(&leased_key(namespace, tenant, &task.queue, task_id))
                        .await;
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
            let mut task: WorkerTask = serde_json::from_str(&raw)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to deserialize task: {e}")))?;
            Self::verify_lease(&task, lease_token)?;

            let queue = task.queue.clone();
            mutate(&mut task, Utc::now());
            let terminal = !task.status.is_active();
            let ttl = terminal.then_some(COMPLETED_TASK_TTL);
            let json = serde_json::to_string(&task)
                .map_err(|e| GatewayError::TaskQueue(format!("failed to serialize task: {e}")))?;
            match self
                .state
                .compare_and_swap(&key, version, &json, ttl)
                .await?
            {
                CasResult::Ok => {
                    let _ = self
                        .state
                        .delete(&leased_key(namespace, tenant, &queue, task_id))
                        .await;
                    return Ok(task);
                }
                CasResult::Conflict { .. } => {}
            }
        }
        Err(GatewayError::TaskQueue(format!(
            "settle contention exhausted for task {task_id}"
        )))
    }

    fn verify_lease(task: &WorkerTask, lease_token: &str) -> Result<(), GatewayError> {
        if task.status != WorkerTaskStatus::Leased {
            return Err(GatewayError::TaskQueue(format!(
                "task {} is not leased (status: {:?})",
                task.task_id, task.status
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

    /// Reclaim expired leases on a queue: re-queue tasks with attempts left,
    /// fail the rest terminally.
    #[allow(clippy::too_many_lines)]
    async fn reclaim_expired_leases(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
    ) -> Result<(), GatewayError> {
        let now = Utc::now();
        let leased = self
            .state
            .scan_keys(
                namespace,
                tenant,
                KeyKind::Custom(QUEUE_LEASED_KIND.into()),
                Some(&format!("{queue}:")),
            )
            .await?;

        for (index_key, expires_ms) in leased {
            let expired = expires_ms
                .parse::<i64>()
                .is_ok_and(|ms| ms <= now.timestamp_millis());
            if !expired {
                continue;
            }
            let Some(task_id) = index_key.rsplit(':').next().map(str::to_owned) else {
                continue;
            };
            let key = task_key(namespace, tenant, &task_id);
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                let _ = self
                    .state
                    .delete(&leased_key(namespace, tenant, queue, &task_id))
                    .await;
                continue;
            };
            let Ok(mut task) = serde_json::from_str::<WorkerTask>(&raw) else {
                continue;
            };
            if !task.lease_expired(now) {
                // Heartbeat raced the index; refresh the index entry.
                if let Some(at) = task.lease_expires_at {
                    let _ = self
                        .state
                        .set(
                            &leased_key(namespace, tenant, queue, &task_id),
                            &at.timestamp_millis().to_string(),
                            None,
                        )
                        .await;
                }
                continue;
            }

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

            let ttl = exhausted.then_some(COMPLETED_TASK_TTL);
            let Ok(json) = serde_json::to_string(&task) else {
                continue;
            };
            match self
                .state
                .compare_and_swap(&key, version, &json, ttl)
                .await?
            {
                CasResult::Ok => {
                    let _ = self
                        .state
                        .delete(&leased_key(namespace, tenant, queue, &task_id))
                        .await;
                    if exhausted {
                        warn!(task_id = %task_id, queue = %queue, "worker task lease expired; attempts exhausted");
                        self.push_task_to_dlq(&task).await;
                        if task.chain_id.is_some() {
                            self.resume_chain_worker_step(
                                &task,
                                Err(task.error.clone().unwrap_or_default()),
                            )
                            .await;
                        }
                        self.route_workflow_task_result(&task).await;
                    } else {
                        self.state
                            .set(
                                &pending_key(namespace, tenant, queue, &task_id),
                                "pending",
                                None,
                            )
                            .await?;
                        debug!(task_id = %task_id, queue = %queue, "expired lease reclaimed; task re-queued");
                    }
                }
                CasResult::Conflict { .. } => {}
            }
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
            dlq.push(
                action,
                task.error
                    .clone()
                    .unwrap_or_else(|| "worker task failed".into()),
                task.attempt,
            )
            .await;
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

            let chain_config = chain_state
                .config_snapshot
                .as_deref()
                .cloned()
                .or_else(|| self.chains.read().get(&chain_state.chain_name).cloned())
                .ok_or_else(|| {
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
            let now = Utc::now();

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
