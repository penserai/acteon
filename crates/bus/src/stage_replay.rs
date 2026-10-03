//! Durable, bounded repair requests executed by the existing leased processor.
use super::{
    BusMessage, CAS_RETRIES, CancellationToken, DateTime, Deserialize, DeserializeOwned, Duration,
    ManagedStageState, ManagedStreamStage, Serialize, StreamCheckpointError, StreamPosition,
    StreamStageError, StreamStageInput, StreamStageOperator, StreamStageProcessError,
    StreamStageProcessor, StreamStageResult, StreamStageTransition, TimeDelta, Utc, Uuid,
    bounded_error,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamReplayStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
impl StreamReplayStatus {
    pub(super) fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamReplayAudit {
    pub request_id: Uuid,
    pub quarantine_id: String,
    pub actor: String,
    pub reason: String,
    pub requested_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub processor_version: String,
    pub position: StreamPosition,
    pub contract_sha256: Option<String>,
    pub original_payload_sha256: String,
    pub repaired_payload_sha256: String,
    pub status: StreamReplayStatus,
    pub attempts: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub completed_generation: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayJob {
    pub audit: StreamReplayAudit,
    pub message: BusMessage,
}
fn reserved_bytes(jobs: &[ReplayJob]) -> Result<usize, StreamCheckpointError> {
    let mut worst = jobs.to_vec();
    for job in &mut worst {
        let a = &mut job.audit;
        a.last_error = Some("\0".repeat(4096)); // JSON control escapes have the worst byte expansion.
        a.finished_at = Some(Utc::now());
        a.next_attempt_at = Some(Utc::now());
        a.attempts = u32::MAX;
        a.completed_generation = Some(u64::MAX);
        a.status = StreamReplayStatus::Completed;
    }
    Ok(serde_json::to_vec(&worst)?
        .len()
        .saturating_add(jobs.len().saturating_mul(128)))
}
impl ManagedStageState {
    pub(super) fn validate_replays(&self) -> Result<(), String> {
        if self.replays.len() > self.config.max_replay_requests
            || reserved_bytes(&self.replays).map_err(|e| e.to_string())?
                > self.config.max_replay_bytes
        {
            return Err("replay audit retention capacity exceeded".into());
        }
        if self
            .replays
            .iter()
            .filter(|j| j.audit.status == StreamReplayStatus::Completed)
            .count() as u64
            != self.counters.replayed_quarantined_records
        {
            return Err("completed replay audits do not match replay counter".into());
        }
        let mut ids = BTreeSet::new();
        let mut active = BTreeSet::new();
        for job in &self.replays {
            let a = &job.audit;
            if !ids.insert(a.request_id)
                || a.request_id.is_nil()
                || a.actor.trim().is_empty()
                || a.actor.len() > 256
                || a.reason.trim().is_empty()
                || a.reason.len() > 4096
                || a.processor_version != self.processor_version
                || a.repaired_payload_sha256 != crate::ingestion::digest(&job.message.payload)
                || a.position.lane.topic != job.message.topic
                || job.message.offset != Some(a.position.offset)
                || job.message.partition != Some(a.position.lane.partition)
                || a.attempts > self.config.max_attempts
            {
                return Err("invalid replay audit definition".into());
            }
            if a.status.active() {
                let Some(original) = self.quarantine.iter().find(|q| q.id == a.quarantine_id)
                else {
                    return Err("active replay lost original input".into());
                };
                if !active.insert(&a.quarantine_id)
                    || original.position != a.position
                    || original.contract_sha256 != a.contract_sha256
                    || crate::ingestion::digest(&original.message.payload)
                        != a.original_payload_sha256
                {
                    return Err("active replay original input mismatch".into());
                }
            }
            let expected = self
                .config
                .input
                .contracts
                .get(&a.position.lane.source)
                .map(|c| &c.sha256);
            if a.contract_sha256.as_ref() != expected
                || a.original_payload_sha256.len() != 64
                || a.last_error.as_ref().is_some_and(|e| e.len() > 4096)
                || a.status == StreamReplayStatus::Running && a.attempts == 0
                || a.finished_at.is_some_and(|at| at < a.requested_at)
                || !a.status.active() && a.next_attempt_at.is_some()
            {
                return Err("invalid replay audit policy or progress".into());
            }
            let terminal = !a.status.active();
            if terminal != a.finished_at.is_some()
                || (a.status == StreamReplayStatus::Completed) != a.completed_generation.is_some()
            {
                return Err("replay audit outcome mismatch".into());
            }
        }
        Ok(())
    }
}
impl StreamStageOperator {
    pub fn replay_audits(&self) -> Vec<StreamReplayAudit> {
        self.coordinator
            .snapshot
            .processing
            .as_ref()
            .unwrap()
            .replays
            .iter()
            .map(|j| j.audit.clone())
            .collect()
    }
    /// Enqueue a repair once. Request IDs bind the actor, reason, input and payload.
    /// Audit history is never evicted automatically to accept another request.
    #[allow(clippy::too_many_lines)] // Keep admission and idempotency binding in one CAS transition.
    pub async fn request_replay(
        &mut self,
        request_id: Uuid,
        quarantine_id: &str,
        actor: &str,
        reason: &str,
        payload: serde_json::Value,
    ) -> Result<StreamReplayAudit, StreamStageError> {
        if request_id.is_nil()
            || actor.trim().is_empty()
            || actor.len() > 256
            || reason.trim().is_empty()
            || reason.len() > 4096
        {
            return Err(StreamStageError::InvalidConfig(
                "require request UUID, actor and bounded nonempty reason".into(),
            ));
        }
        for _ in 0..CAS_RETRIES {
            self.coordinator.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next
                .processing
                .as_mut()
                .ok_or(StreamStageError::MissingManagedState)?;
            if let Some(job) = m.replays.iter().find(|j| j.audit.request_id == request_id) {
                let a = &job.audit;
                if a.quarantine_id != quarantine_id
                    || a.actor != actor
                    || a.reason != reason
                    || job.message.payload != payload
                {
                    return Err(StreamStageError::ReplayConflict);
                }
                return Ok(a.clone());
            }
            if m.replays
                .iter()
                .any(|j| j.audit.quarantine_id == quarantine_id && j.audit.status.active())
            {
                return Err(StreamStageError::ReplayConflict);
            }
            let original = m
                .quarantine
                .iter()
                .find(|q| q.id == quarantine_id)
                .ok_or(StreamStageError::ReplayConflict)?;
            if serde_json::to_vec(&payload)
                .map_err(StreamCheckpointError::from)?
                .len()
                > m.config.max_batch_bytes
            {
                return Err(StreamStageError::InvalidConfig(
                    "repair exceeds input byte bound".into(),
                ));
            }
            if let Some(contract) = m.config.input.contracts.get(&original.position.lane.source) {
                let validator = contract
                    .compile()
                    .map_err(StreamStageError::InvalidConfig)?;
                if !validator.is_valid(&payload) {
                    return Err(StreamStageError::InvalidConfig(
                        "repair violates pinned consume contract".into(),
                    ));
                }
            }
            let audit = StreamReplayAudit {
                request_id,
                quarantine_id: quarantine_id.into(),
                actor: actor.into(),
                reason: reason.into(),
                requested_at: Utc::now(),
                finished_at: None,
                processor_version: m.processor_version.clone(),
                position: original.position.clone(),
                contract_sha256: original.contract_sha256.clone(),
                original_payload_sha256: crate::ingestion::digest(&original.message.payload),
                repaired_payload_sha256: crate::ingestion::digest(&payload),
                status: StreamReplayStatus::Pending,
                attempts: 0,
                next_attempt_at: None,
                last_error: None,
                completed_generation: None,
            };
            let mut message = original.message.clone();
            message.payload = payload.clone();
            if serde_json::to_vec(&message)
                .map_err(StreamCheckpointError::from)?
                .len()
                > m.config.max_batch_bytes
            {
                return Err(StreamStageError::InvalidConfig(
                    "repair envelope exceeds input byte bound".into(),
                ));
            }
            m.replays.push(ReplayJob {
                audit: audit.clone(),
                message,
            });
            if m.replays.len() > m.config.max_replay_requests
                || reserved_bytes(&m.replays)? > m.config.max_replay_bytes
            {
                return Err(StreamStageError::ReplayCapacity);
            }
            match self.coordinator.persist(next).await {
                Err(StreamCheckpointError::Conflict { .. }) => {}
                r => {
                    r?;
                    return Ok(audit);
                }
            }
        }
        Err(StreamStageError::Fenced)
    }
}
impl<S, O> ManagedStreamStage<S, O>
where
    S: Clone + Serialize + DeserializeOwned + Send + Sync,
    O: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    /// Explicit worker entry point, requiring no live Kafka consumer. The normal
    /// process loop also drains ready replays before receiving a source batch.
    pub async fn replay_once<I, P>(
        &mut self,
        processor: &P,
        cancel: &CancellationToken,
    ) -> Result<StreamStageResult, StreamStageError>
    where
        I: DeserializeOwned + Send + Sync,
        P: StreamStageProcessor<I, S, O>,
    {
        if cancel.is_cancelled() {
            return Ok(StreamStageResult::Cancelled {
                durable_generation: None,
            });
        }
        let token = match self.claim().await? {
            Ok(t) => t,
            Err(r) => return Ok(r),
        };
        if let Some(result) = self.process_replay(token, processor, cancel).await? {
            return Ok(result);
        }
        self.release(token).await?;
        Ok(StreamStageResult::Idle)
    }
    #[allow(clippy::too_many_lines)] // Keep lease, attempt persistence and callback boundaries explicit.
    pub(super) async fn process_replay<I, P>(
        &mut self,
        token: Uuid,
        processor: &P,
        cancel: &CancellationToken,
    ) -> Result<Option<StreamStageResult>, StreamStageError>
    where
        I: DeserializeOwned + Send + Sync,
        P: StreamStageProcessor<I, S, O>,
    {
        let Some(index) = self.managed().replays.iter().position(|j| {
            j.audit.status.active() && j.audit.next_attempt_at.is_none_or(|at| at <= Utc::now())
        }) else {
            return Ok(None);
        };
        let job = self.managed().replays[index].clone();
        if job.audit.attempts >= self.config.max_attempts {
            return self
                .finish_replay(
                    token,
                    job.audit.request_id,
                    Err(StreamStageProcessError::Permanent(
                        "replay attempt budget exhausted after interruption".into(),
                    )),
                )
                .await
                .map(Some);
        }
        let mut started = false;
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            let mut next = self.coordinator.snapshot.clone();
            let a = &mut next
                .processing
                .as_mut()
                .unwrap()
                .replays
                .iter_mut()
                .find(|j| j.audit.request_id == job.audit.request_id)
                .ok_or(StreamStageError::ReplayConflict)?
                .audit;
            a.attempts += 1;
            a.status = StreamReplayStatus::Running;
            a.next_attempt_at = None;
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                r => {
                    r?;
                    started = true;
                    break;
                }
            }
        }
        if !started {
            return Err(StreamStageError::Fenced);
        }
        let decoded: Result<I, String> = if self
            .validators
            .get(&job.audit.position.lane.source)
            .is_some_and(|v| !v.is_valid(&job.message.payload))
        {
            Err("repair violates pinned consume contract".into())
        } else {
            serde_json::from_value(job.message.payload.clone())
                .map_err(|_| "repair cannot be decoded into processor input type".into())
        };
        let result = match decoded {
            Err(reason) => Err(StreamStageProcessError::Permanent(reason)),
            Ok(payload) => {
                let input = StreamStageInput {
                    position: job.audit.position,
                    message: job.message,
                    payload,
                };
                let inputs = [input];
                tokio::select! { biased;
                    ()=cancel.cancelled()=>{ self.release(token).await?; return Ok(Some(StreamStageResult::Cancelled {durable_generation:None})); },
                    r=tokio::time::timeout(Duration::from_millis(self.config.processing_timeout_ms),processor.process(self.checkpoint().state().clone(), &inputs)) => match r {Ok(r)=>r,Err(_)=>Err(StreamStageProcessError::Retryable("replay processing timeout".into()))}
                }
            }
        };
        let result = match result {
            Ok(t)
                if t.outputs.len() > self.config.max_outputs_per_batch
                    || serde_json::to_vec(&t.outputs)
                        .map_err(StreamCheckpointError::from)?
                        .len()
                        > self.config.max_output_bytes =>
            {
                Err(StreamStageProcessError::Permanent(
                    "replay processor exceeded output bounds".into(),
                ))
            }
            r => r,
        };
        let result = match result {
            Ok(t) => {
                match self
                    .coordinator
                    .prepare_checkpoint(t.state.clone(), [], t.outputs.clone())
                {
                    Ok(_) => Ok(t),
                    Err(_) => Err(StreamStageProcessError::Permanent(
                        "replay processor returned an invalid checkpoint transition".into(),
                    )),
                }
            }
            r => r,
        };
        self.finish_replay(token, job.audit.request_id, result)
            .await
            .map(Some)
    }
    async fn finish_replay(
        &mut self,
        token: Uuid,
        id: Uuid,
        result: Result<StreamStageTransition<S, O>, StreamStageProcessError>,
    ) -> Result<StreamStageResult, StreamStageError> {
        let revision = self.managed().revision;
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            if self.managed().revision != revision {
                return Err(StreamStageError::Fenced);
            }
            let mut next = match &result {
                Ok(t) => {
                    self.coordinator
                        .prepare_checkpoint(t.state.clone(), [], t.outputs.clone())?
                }
                Err(_) => self.coordinator.snapshot.clone(),
            };
            let generation = next.generation();
            let m = next.processing.as_mut().unwrap();
            let index = m
                .replays
                .iter()
                .position(|j| j.audit.request_id == id)
                .ok_or(StreamStageError::ReplayConflict)?;
            let a = &mut m.replays[index].audit;
            let outcome = match &result {
                Ok(_) => {
                    a.status = StreamReplayStatus::Completed;
                    a.finished_at = Some(Utc::now());
                    a.completed_generation = Some(generation);
                    a.next_attempt_at = None;
                    a.last_error = None;
                    let quarantine_id = a.quarantine_id.clone();
                    m.quarantine.retain(|q| q.id != quarantine_id);
                    m.counters.replayed_quarantined_records = m
                        .counters
                        .replayed_quarantined_records
                        .checked_add(1)
                        .ok_or(StreamCheckpointError::GenerationOverflow)?;
                    m.revision = m
                        .revision
                        .checked_add(1)
                        .ok_or(StreamCheckpointError::GenerationOverflow)?;
                    StreamStageResult::ReplayCompleted {
                        request_id: id,
                        generation,
                    }
                }
                Err(e) => {
                    a.last_error = Some(bounded_error(e.to_string()));
                    if matches!(e, StreamStageProcessError::Retryable(_))
                        && a.attempts < self.config.max_attempts
                    {
                        let at = Utc::now()
                            + TimeDelta::milliseconds(
                                i64::try_from(self.config.backoff(a.attempts)).unwrap(),
                            );
                        a.status = StreamReplayStatus::Pending;
                        a.next_attempt_at = Some(at);
                        StreamStageResult::ReplayRetryScheduled {
                            request_id: id,
                            next_attempt_at: at,
                        }
                    } else {
                        a.status = StreamReplayStatus::Failed;
                        a.finished_at = Some(Utc::now());
                        a.next_attempt_at = None;
                        StreamStageResult::ReplayFailed { request_id: id }
                    }
                }
            };
            m.lease = None;
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                r => {
                    r?;
                    return Ok(outcome);
                }
            }
        }
        Err(StreamStageError::Fenced)
    }
}
