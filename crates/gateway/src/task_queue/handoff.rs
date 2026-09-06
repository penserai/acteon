//! Terminal task records are their own outbox. Delivery progress and the worker
//! result share one CAS; no separate outbox insert can be lost after settlement.

use super::{
    COMPLETED_TASK_TTL, Gateway, GatewayError, MAX_CAS_ATTEMPTS, WORKER_TASK_KIND, task_key,
};
use acteon_core::{Action, WorkerTask, WorkerTaskHandoff, WorkerTaskStatus};
use acteon_state::{CasResult, KeyKind};
use std::{future::Future, time::Duration};
use tracing::warn;

const LEASE_SECONDS: i64 = 60;
#[derive(Clone, Copy)]
enum Destination {
    Chain,
    Workflow,
    Dlq,
}
impl Gateway {
    pub(super) fn prepare_worker_handoff(&self, task: &mut WorkerTask) -> Option<Duration> {
        let mut handoff = WorkerTaskHandoff {
            delivery_id: uuid::Uuid::new_v4().to_string(),
            chain_pending: task.chain_id.is_some(),
            workflow_pending: task.workflow_execution_id.is_some(),
            dlq_pending: task.status == WorkerTaskStatus::Failed && self.dlq.is_some(),
            lease_token: None,
            lease_expires_at: None,
            completed_at: None,
        };
        let ttl = (!handoff.pending()).then_some(COMPLETED_TASK_TTL);
        if ttl.is_some() {
            handoff.completed_at = Some(self.clock.now());
        }
        task.handoff = Some(handoff);
        ttl
    }

    pub(super) async fn try_worker_handoff(&self, task: &WorkerTask) {
        if let Err(error) =
            Box::pin(self.deliver_worker_handoff(&task.namespace, &task.tenant, &task.task_id))
                .await
        {
            warn!(%error, task_id = %task.task_id, "worker result handoff retained for retry");
        }
    }

    /// Retry unfinished terminal task handoffs across scopes. Successful
    /// destinations are acknowledged independently. Invalid records and failed
    /// deliveries remain stored; the first error is returned after other records
    /// have been attempted. Legacy terminal rows without handoff metadata are
    /// not replayed because their previous delivery cannot be inferred.
    pub async fn reconcile_worker_handoffs(&self) -> Result<usize, GatewayError> {
        let rows = self
            .state
            .scan_keys_by_kind(KeyKind::Custom(WORKER_TASK_KIND.into()))
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for (key, raw) in rows {
            let parts: Vec<_> = key.splitn(4, ':').collect();
            if parts.len() != 4 || parts[2] != WORKER_TASK_KIND {
                continue;
            }
            let task = match self.decode_worker_task(&raw, parts[0], parts[1], parts[3]) {
                Ok(task) => task,
                Err(error) => {
                    warn!(%error, "invalid worker handoff record retained");
                    continue;
                }
            };
            if task.status.is_active()
                || task
                    .handoff
                    .as_ref()
                    .is_none_or(|handoff| handoff.completed_at.is_some())
            {
                continue;
            }
            match Box::pin(self.deliver_worker_handoff(parts[0], parts[1], parts[3])).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(completed),
        }
    }

    async fn claim_worker_handoff(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<Option<WorkerTask>, GatewayError> {
        let key = task_key(namespace, tenant, id);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                return Ok(None);
            };
            let mut task = self.decode_worker_task(&raw, namespace, tenant, id)?;
            if task.status.is_active() {
                return Ok(None);
            }
            let Some(handoff) = task.handoff.as_mut() else {
                return Ok(None);
            };
            let now = self.clock.now();
            if handoff.completed_at.is_some() || handoff.lease_expires_at.is_some_and(|at| at > now)
            {
                return Ok(None);
            }
            handoff.lease_token = Some(uuid::Uuid::new_v4().to_string());
            handoff.lease_expires_at = Some(now + chrono::Duration::seconds(LEASE_SECONDS));
            if self
                .state
                .compare_and_swap(&key, version, &self.encode_worker_task(&task)?, None)
                .await?
                == CasResult::Ok
            {
                return Ok(Some(task));
            }
        }
        Err(GatewayError::TaskQueue(
            "worker handoff claim contention".into(),
        ))
    }

    /// Only the current, unexpired owner can renew or acknowledge delivery.
    async fn update_worker_handoff(
        &self,
        task: &WorkerTask,
        token: &str,
        destination: Option<Destination>,
        release: bool,
    ) -> Result<(), GatewayError> {
        let key = task_key(&task.namespace, &task.tenant, &task.task_id);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Some((raw, version)) = self.state.get_versioned(&key).await? else {
                break;
            };
            let mut current =
                self.decode_worker_task(&raw, &task.namespace, &task.tenant, &task.task_id)?;
            let Some(handoff) = current.handoff.as_mut() else {
                break;
            };
            let now = self.clock.now();
            if handoff.lease_token.as_deref() != Some(token)
                || handoff.lease_expires_at.is_none_or(|at| at <= now)
            {
                break;
            }
            if let Some(destination) = destination {
                match destination {
                    Destination::Chain => handoff.chain_pending = false,
                    Destination::Workflow => handoff.workflow_pending = false,
                    Destination::Dlq => handoff.dlq_pending = false,
                }
            }
            let pending = handoff.pending();
            if release {
                handoff.lease_token = None;
                handoff.lease_expires_at = None;
                if !pending {
                    handoff.completed_at = Some(now);
                }
            } else {
                handoff.lease_expires_at = Some(now + chrono::Duration::seconds(LEASE_SECONDS));
            }
            // Retain even fully delivered progress until the claim is released.
            // A lost acknowledgement is then discoverable through completed flags.
            let ttl = (release && !pending).then_some(COMPLETED_TASK_TTL);
            if self
                .state
                .compare_and_swap(&key, version, &self.encode_worker_task(&current)?, ttl)
                .await?
                == CasResult::Ok
            {
                return Ok(());
            }
        }
        Err(GatewayError::TaskQueue(
            "worker handoff ownership lost".into(),
        ))
    }

    async fn keep_worker_handoff_lease(
        &self,
        task: &WorkerTask,
        token: &str,
        delivery: impl Future<Output = Result<(), GatewayError>>,
    ) -> Result<(), GatewayError> {
        tokio::pin!(delivery);
        loop {
            tokio::select! {
                result = &mut delivery => return result,
                () = self.clock.sleep(Duration::from_secs(20)) => self.update_worker_handoff(task, token, None, false).await?,
            }
        }
    }

    async fn deliver_worker_handoff(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<bool, GatewayError> {
        let Some(task) = self.claim_worker_handoff(namespace, tenant, id).await? else {
            return Ok(false);
        };
        let handoff = task.handoff.as_ref().expect("claimed handoff");
        let token = handoff.lease_token.as_deref().expect("claimed token");
        let mut first_error = None;
        for (destination, pending) in [
            (Destination::Chain, handoff.chain_pending),
            (Destination::Workflow, handoff.workflow_pending),
            (Destination::Dlq, handoff.dlq_pending),
        ] {
            if !pending {
                continue;
            }
            let delivery = async {
                match destination {
                    Destination::Chain => {
                        let outcome = if task.status == WorkerTaskStatus::Completed {
                            Ok(task.result.clone().unwrap_or_default())
                        } else {
                            Err(task
                                .error
                                .clone()
                                .unwrap_or_else(|| "worker task failed".into()))
                        };
                        self.resume_chain_worker_step(&task, outcome).await
                    }
                    Destination::Workflow => self.settle_workflow_task(&task).await,
                    Destination::Dlq => self.deliver_worker_dlq(&task).await,
                }
            };
            match self.keep_worker_handoff_lease(&task, token, delivery).await {
                Ok(()) => {
                    self.update_worker_handoff(&task, token, Some(destination), false)
                        .await?;
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        self.update_worker_handoff(&task, token, None, true).await?;
        match first_error {
            Some(error) => Err(error),
            None => Ok(true),
        }
    }

    async fn deliver_worker_dlq(&self, task: &WorkerTask) -> Result<(), GatewayError> {
        let sink = self.dlq.as_ref().ok_or_else(|| {
            GatewayError::TaskQueue("worker handoff requires its configured DLQ sink".into())
        })?;
        let mut action = Action::new(
            &*task.namespace,
            &*task.tenant,
            format!("queue:{}", task.queue),
            &task.action_type,
            task.payload.clone(),
        );
        action.id = task
            .handoff
            .as_ref()
            .expect("DLQ handoff")
            .delivery_id
            .clone()
            .into();
        action.created_at = task.updated_at;
        sink.push(
            action,
            task.error
                .clone()
                .unwrap_or_else(|| "worker task failed".into()),
            task.attempt,
        )
        .await
        .map_err(|_| GatewayError::TaskQueue("worker DLQ handoff was not acknowledged".into()))
    }
}
