//! Leased typed processing: receive, validate, checkpoint state/outputs, acknowledge.
//! Completed positions suppress reprocessing after a crash. Calls interrupted before
//! checkpointing may run again; processors should avoid non-idempotent side effects.
use crate::{
    BusMessage, StreamCheckpointCoordinator, StreamCheckpointError, StreamCheckpointSnapshot,
    StreamInputFailure, StreamInputPolicy, StreamOutboxEntry, StreamPoisonPolicy, StreamPosition,
    StreamQuarantinedInput, StreamStageRecord, StreamStageSource, StreamStageSourceError,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const CAS_RETRIES: usize = 8;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStageConfig {
    #[serde(default, skip_serializing_if = "StreamInputPolicy::is_default")]
    pub input: StreamInputPolicy,
    pub max_batch_records: usize,
    pub max_batch_bytes: usize,
    pub max_outputs_per_batch: usize,
    pub max_output_bytes: usize,
    /// Reserve room for one maximum-sized output batch before receiving input.
    pub outbox_high_watermark: usize,
    pub lease_ms: u64,
    pub receive_timeout_ms: u64,
    pub processing_timeout_ms: u64,
    pub source_timeout_ms: u64,
    pub storage_timeout_ms: u64,
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub poll_interval_ms: u64,
}
impl Default for StreamStageConfig {
    fn default() -> Self {
        Self {
            input: StreamInputPolicy::default(),
            max_batch_records: 64,
            max_batch_bytes: 8 * 1024 * 1024,
            max_outputs_per_batch: 64,
            max_output_bytes: 8 * 1024 * 1024,
            outbox_high_watermark: 5000,
            lease_ms: 60_000,
            receive_timeout_ms: 5000,
            processing_timeout_ms: 20_000,
            source_timeout_ms: 5000,
            storage_timeout_ms: 5000,
            max_attempts: 5,
            initial_backoff_ms: 1000,
            max_backoff_ms: 30_000,
            poll_interval_ms: 100,
        }
    }
}
impl StreamStageConfig {
    fn validate(&self) -> Result<(), String> {
        self.input.validate()?;
        let budget = self
            .receive_timeout_ms
            .checked_add(self.processing_timeout_ms)
            .and_then(|n| n.checked_add(self.source_timeout_ms.saturating_mul(2)))
            .and_then(|n| n.checked_add(self.storage_timeout_ms.saturating_mul(3)));
        if !(1..=100_000).contains(&self.max_batch_records)
            || self.max_batch_bytes == 0
            || self.max_outputs_per_batch == 0
            || self.max_output_bytes == 0
            || self.outbox_high_watermark < self.max_outputs_per_batch
            || self.receive_timeout_ms == 0
            || self.processing_timeout_ms == 0
            || self.source_timeout_ms == 0
            || self.storage_timeout_ms == 0
            || self.max_attempts == 0
            || self.initial_backoff_ms == 0
            || self.max_backoff_ms < self.initial_backoff_ms
            || self.poll_interval_ms == 0
            || budget.is_none_or(|b| self.lease_ms <= b)
        {
            return Err("require positive bounds/timeouts, output headroom, lease exceeding operation budgets, and bounded attempts/backoff".into());
        }
        for n in [self.lease_ms, self.max_backoff_ms] {
            if i64::try_from(n)
                .ok()
                .and_then(TimeDelta::try_milliseconds)
                .and_then(|delta| Utc::now().checked_add_signed(delta))
                .is_none()
            {
                return Err("unsupported timestamp duration".into());
            }
        }
        Ok(())
    }
    fn backoff(&self, attempt: u32) -> u64 {
        self.initial_backoff_ms
            .saturating_mul(
                1_u64
                    .checked_shl(attempt.saturating_sub(1))
                    .unwrap_or(u64::MAX),
            )
            .min(self.max_backoff_ms)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStageCounters {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub quarantined_records: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub discarded_quarantined_records: u64,
    pub attempts: u64,
    pub completed_batches: u64,
    pub processed_records: u64,
    pub recovered_records: u64,
    pub retries_scheduled: u64,
    pub failures: u64,
    pub recovered_leases: u64,
    pub acknowledgement_failures: u64,
    pub total_processing_ms: u64,
    pub last_processing_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStageMetrics {
    pub counters: StreamStageCounters,
    pub checkpoint_generation: u64,
    pub pending_outputs: usize,
    pub quarantined_records: usize,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub halted: bool,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageLease {
    token: Uuid,
    worker: String,
    expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageRetry {
    anchor: StreamPosition,
    attempts: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    terminal: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedStageState {
    config: StreamStageConfig,
    processor_version: String,
    source_identity: String,
    revision: u64,
    lease: Option<StageLease>,
    retry: Option<StageRetry>,
    counters: StreamStageCounters,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    quarantine: Vec<StreamQuarantinedInput>,
}
impl ManagedStageState {
    pub(crate) fn requires_v4(&self) -> bool {
        !self.config.input.is_default()
            || !self.quarantine.is_empty()
            || self.counters.quarantined_records > 0
    }
    fn validate_quarantine(&self) -> Result<(), String> {
        if !quarantine_fits(&self.quarantine, &self.config.input).map_err(|e| e.to_string())? {
            return Err("quarantine retention capacity exceeded".into());
        }
        let mut ids = BTreeSet::new();
        let mut positions = BTreeSet::new();
        for entry in &self.quarantine {
            let p = &entry.position;
            if Uuid::parse_str(&entry.id).is_err()
                || !ids.insert(&entry.id)
                || !positions.insert((&p.lane, p.offset))
                || p.offset < 0
                || p.lane.partition < 0
                || p.lane.source.trim().is_empty()
                || p.lane.consumer_group.trim().is_empty()
                || p.lane.topic != entry.message.topic
                || entry.message.partition != Some(p.lane.partition)
                || entry.message.offset != Some(p.offset)
                || entry.reason.is_empty()
                || entry.reason.len() > 4096
            {
                return Err("invalid quarantine envelope, identity, or diagnostic".into());
            }
            let expected = self
                .config
                .input
                .contracts
                .get(&p.lane.source)
                .map(|c| &c.sha256);
            if entry.contract_sha256.as_ref() != expected
                || (entry.failure == StreamInputFailure::SchemaViolation && expected.is_none())
            {
                return Err("quarantine contract mismatch".into());
            }
        }
        if !self.quarantine.is_empty()
            && self.config.input.poison_policy != StreamPoisonPolicy::Quarantine
        {
            return Err("quarantine entries require quarantine policy".into());
        }
        if self
            .counters
            .quarantined_records
            .checked_sub(self.counters.discarded_quarantined_records)
            != Some(u64::try_from(self.quarantine.len()).map_err(|e| e.to_string())?)
        {
            return Err("quarantine counters do not match retained entries".into());
        }
        Ok(())
    }
    pub(crate) fn validate_positions(&self, positions: &[StreamPosition]) -> Result<(), String> {
        if self.quarantine.iter().any(|entry| {
            !positions
                .iter()
                .any(|p| p.lane == entry.position.lane && p.offset >= entry.position.offset)
        }) {
            return Err("quarantine input progress is not durable".into());
        }
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.config.validate()?;
        self.validate_quarantine()?;
        if self.processor_version.trim().is_empty()
            || self.processor_version.len() > 4096
            || self.source_identity.trim().is_empty()
            || self.source_identity.len() > 16_384
        {
            return Err("empty or oversized stage definition".into());
        }
        if self
            .lease
            .as_ref()
            .is_some_and(|l| l.worker.trim().is_empty() || l.worker.len() > 256)
        {
            return Err("invalid worker identity".into());
        }
        if let Some(r) = &self.retry
            && (r.attempts == 0
                || r.attempts > self.config.max_attempts
                || r.anchor.offset < 0
                || r.anchor.lane.partition < 0
                || r.anchor.lane.source.trim().is_empty()
                || r.anchor.lane.topic.trim().is_empty()
                || r.anchor.lane.consumer_group.trim().is_empty()
                || r.last_error.as_ref().is_some_and(|e| e.len() > 4096)
                || r.terminal
                    && (r.next_attempt_at.is_some()
                        || r.last_error.is_none()
                        || self.lease.is_some()))
        {
            return Err("invalid retry anchor, budget, or terminal state".into());
        }
        Ok(())
    }
}
/// Typed payload plus transport metadata and a validated ordered source position.
#[derive(Debug, Clone)]
pub struct StreamStageInput<I> {
    pub position: StreamPosition,
    pub message: BusMessage,
    pub payload: I,
}
#[derive(Debug, Clone)]
pub struct StreamStageTransition<S, O> {
    pub state: S,
    pub outputs: Vec<StreamOutboxEntry<O>>,
}
#[derive(Debug, Error)]
pub enum StreamStageProcessError {
    #[error("retryable processor failure: {0}")]
    Retryable(String),
    #[error("permanent processor failure: {0}")]
    Permanent(String),
}
/// Processors return a new state and stable-keyed outputs. They do not commit
/// offsets, write the checkpoint, or acknowledge their own inputs.
#[async_trait]
pub trait StreamStageProcessor<I, S, O>: Send + Sync {
    async fn process(
        &self,
        state: S,
        inputs: &[StreamStageInput<I>],
    ) -> Result<StreamStageTransition<S, O>, StreamStageProcessError>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamStageResult {
    Idle,
    Busy,
    Backpressured,
    Completed {
        generation: u64,
        processed: usize,
        recovered: usize,
    },
    RetryScheduled {
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
    },
    Halted {
        attempts: u32,
        reason: String,
    },
    Cancelled {
        durable_generation: Option<u64>,
    },
}
#[derive(Debug, Error)]
pub enum StreamStageError {
    #[error("invalid stream stage: {0}")]
    InvalidConfig(String),
    #[error("stage definition or persisted policy changed")]
    DefinitionMismatch,
    #[error("managed processing metadata is missing")]
    MissingManagedState,
    #[error("processing lease was lost or expired")]
    Fenced,
    #[error("source batch violated its ordered receipt contract: {0}")]
    InvalidBatch(String),
    #[error("source did not redeliver the outstanding retry anchor")]
    RetryAnchorMissing,
    #[error("{operation} timed out; reload durable state before retrying")]
    Timeout { operation: &'static str },
    #[error(transparent)]
    Checkpoint(#[from] StreamCheckpointError),
    #[error(transparent)]
    Source(#[from] StreamStageSourceError),
    #[error("source acknowledgement failed after durable generation {generation}: {error}")]
    Acknowledgement {
        generation: u64,
        error: StreamStageSourceError,
    },
}

pub struct ManagedStreamStage<S, O> {
    coordinator: StreamCheckpointCoordinator<S, O>,
    worker: String,
    config: StreamStageConfig,
    processor_version: String,
    source_identity: String,
    validators: BTreeMap<String, jsonschema::Validator>,
}
impl<S, O> ManagedStreamStage<S, O>
where
    S: Clone + Serialize + DeserializeOwned + Send + Sync,
    O: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    pub async fn initialize(
        mut coordinator: StreamCheckpointCoordinator<S, O>,
        worker: impl Into<String>,
        processor_version: impl Into<String>,
        source_identity: impl Into<String>,
        config: StreamStageConfig,
    ) -> Result<Self, StreamStageError> {
        config.validate().map_err(StreamStageError::InvalidConfig)?;
        let validators = config
            .input
            .contracts
            .iter()
            .map(|(name, contract)| {
                Ok((
                    name.clone(),
                    contract
                        .compile()
                        .map_err(StreamStageError::InvalidConfig)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, StreamStageError>>()?;
        let worker = worker.into();
        let processor_version = processor_version.into();
        let source_identity = source_identity.into();
        if worker.trim().is_empty()
            || worker.len() > 256
            || config.outbox_high_watermark > coordinator.snapshot().config().max_pending_outputs
        {
            return Err(StreamStageError::InvalidConfig(
                "invalid worker or outbox watermark exceeds checkpoint capacity".into(),
            ));
        }
        for _ in 0..CAS_RETRIES {
            if let Some(existing) = &coordinator.snapshot.processing {
                if existing.config != config
                    || existing.processor_version != processor_version
                    || existing.source_identity != source_identity
                {
                    return Err(StreamStageError::DefinitionMismatch);
                }
                return Ok(Self {
                    coordinator,
                    worker,
                    config,
                    processor_version,
                    source_identity,
                    validators,
                });
            }
            let mut next = coordinator.snapshot.clone();
            next.processing = Some(ManagedStageState {
                config: config.clone(),
                processor_version: processor_version.clone(),
                source_identity: source_identity.clone(),
                revision: 0,
                lease: None,
                retry: None,
                counters: StreamStageCounters::default(),
                quarantine: Vec::new(),
            });
            match tokio::time::timeout(
                Duration::from_millis(config.storage_timeout_ms),
                coordinator.persist(next),
            )
            .await
            {
                Ok(Ok(())) => {
                    return Ok(Self {
                        coordinator,
                        worker,
                        config,
                        processor_version,
                        source_identity,
                        validators,
                    });
                }
                Ok(Err(StreamCheckpointError::Conflict { .. })) => tokio::time::timeout(
                    Duration::from_millis(config.storage_timeout_ms),
                    coordinator.reload(),
                )
                .await
                .map_err(|_| StreamStageError::Timeout {
                    operation: "initialize reload",
                })??,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    return Err(StreamStageError::Timeout {
                        operation: "initialize",
                    });
                }
            }
        }
        Err(StreamStageError::Fenced)
    }
    pub fn checkpoint(&self) -> &StreamCheckpointSnapshot<S, O> {
        self.coordinator.snapshot()
    }
    pub fn into_checkpoint(self) -> StreamCheckpointCoordinator<S, O> {
        self.coordinator
    }
    /// Reload and inspect retained original inputs. Receipt capabilities are never exposed.
    pub async fn quarantined_inputs(
        &mut self,
    ) -> Result<Vec<StreamQuarantinedInput>, StreamStageError> {
        self.reload().await?;
        Ok(self.managed().quarantine.clone())
    }
    /// Explicitly remove a retained input. Does not rewind Kafka or invoke the processor.
    pub async fn discard_quarantined_input(&mut self, id: &str) -> Result<bool, StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next.processing.as_mut().unwrap();
            let Some(index) = m.quarantine.iter().position(|e| e.id == id) else {
                return Ok(false);
            };
            m.quarantine.remove(index);
            m.counters.discarded_quarantined_records = m
                .counters
                .discarded_quarantined_records
                .checked_add(1)
                .ok_or(StreamCheckpointError::GenerationOverflow)?;
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                result => {
                    result?;
                    return Ok(true);
                }
            }
        }
        Err(StreamStageError::Fenced)
    }
    fn managed(&self) -> &ManagedStageState {
        self.coordinator
            .snapshot
            .processing
            .as_ref()
            .expect("initialized stage")
    }
    async fn reload(&mut self) -> Result<(), StreamStageError> {
        tokio::time::timeout(
            Duration::from_millis(self.config.storage_timeout_ms),
            self.coordinator.reload(),
        )
        .await
        .map_err(|_| StreamStageError::Timeout {
            operation: "reload",
        })??;
        let m = self
            .coordinator
            .snapshot
            .processing
            .as_ref()
            .ok_or(StreamStageError::MissingManagedState)?;
        if m.config != self.config
            || m.processor_version != self.processor_version
            || m.source_identity != self.source_identity
        {
            return Err(StreamStageError::DefinitionMismatch);
        }
        Ok(())
    }
    pub async fn metrics(&mut self) -> Result<StreamStageMetrics, StreamStageError> {
        self.reload().await?;
        let m = self.managed();
        Ok(StreamStageMetrics {
            counters: m.counters.clone(),
            checkpoint_generation: self.checkpoint().generation(),
            pending_outputs: self.checkpoint().pending_outputs().len(),
            quarantined_records: m.quarantine.len(),
            lease_expires_at: m.lease.as_ref().map(|l| l.expires_at),
            next_attempt_at: m.retry.as_ref().and_then(|r| r.next_attempt_at),
            halted: m.retry.as_ref().is_some_and(|r| r.terminal),
            last_error: m.retry.as_ref().and_then(|r| r.last_error.clone()),
        })
    }
    fn owned(&self, token: Uuid) -> Result<(), StreamStageError> {
        if self
            .managed()
            .lease
            .as_ref()
            .is_some_and(|l| l.token == token && l.expires_at > Utc::now())
        {
            Ok(())
        } else {
            Err(StreamStageError::Fenced)
        }
    }
    async fn persist(
        &mut self,
        next: StreamCheckpointSnapshot<S, O>,
    ) -> Result<(), StreamStageError> {
        tokio::time::timeout(
            Duration::from_millis(self.config.storage_timeout_ms),
            self.coordinator.persist(next),
        )
        .await
        .map_err(|_| StreamStageError::Timeout {
            operation: "checkpoint",
        })??;
        Ok(())
    }
    async fn release(&mut self, token: Uuid) -> Result<(), StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            let mut next = self.coordinator.snapshot.clone();
            next.processing.as_mut().unwrap().lease = None;
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                r => return r,
            }
        }
        Err(StreamStageError::Fenced)
    }
    async fn claim(&mut self) -> Result<Result<Uuid, StreamStageResult>, StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            let now = Utc::now();
            let m = self.managed();
            if m.lease.as_ref().is_some_and(|l| l.expires_at > now) {
                return Ok(Err(StreamStageResult::Busy));
            }
            if let Some(r) = &m.retry {
                if r.terminal {
                    return Ok(Err(StreamStageResult::Halted {
                        attempts: r.attempts,
                        reason: r.last_error.clone().unwrap_or_default(),
                    }));
                }
                if r.attempts >= self.config.max_attempts {
                    let mut next = self.coordinator.snapshot.clone();
                    let m = next.processing.as_mut().unwrap();
                    m.lease = None;
                    let r = m.retry.as_mut().unwrap();
                    r.terminal = true;
                    r.next_attempt_at = None;
                    r.last_error =
                        Some("attempt budget exhausted after interrupted processing".into());
                    match self.persist(next).await {
                        Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict {
                            ..
                        })) => continue,
                        r => r?,
                    }
                    continue;
                }
                if let Some(at) = r.next_attempt_at.filter(|at| *at > now) {
                    return Ok(Err(StreamStageResult::RetryScheduled {
                        attempts: r.attempts,
                        next_attempt_at: at,
                    }));
                }
            }
            if self
                .checkpoint()
                .pending_outputs()
                .len()
                .saturating_add(self.config.max_outputs_per_batch)
                > self.config.outbox_high_watermark
            {
                return Ok(Err(StreamStageResult::Backpressured));
            }
            let token = Uuid::new_v4();
            let mut next = self.coordinator.snapshot.clone();
            let m = next.processing.as_mut().unwrap();
            if m.lease.is_some() {
                m.counters.recovered_leases = m.counters.recovered_leases.saturating_add(1);
            }
            m.lease = Some(StageLease {
                token,
                worker: self.worker.clone(),
                expires_at: now
                    + TimeDelta::milliseconds(i64::try_from(self.config.lease_ms).unwrap()),
            });
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {
                    continue;
                }
                r => r?,
            }
            return Ok(Ok(token));
        }
        Err(StreamStageError::Fenced)
    }
    async fn begin_attempt(
        &mut self,
        token: Uuid,
        anchor: &StreamPosition,
    ) -> Result<(), StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next.processing.as_mut().unwrap();
            let attempts = m.retry.as_ref().map_or(1, |r| r.attempts.saturating_add(1));
            m.retry = Some(StageRetry {
                anchor: anchor.clone(),
                attempts,
                next_attempt_at: None,
                last_error: None,
                terminal: false,
            });
            m.counters.attempts = m.counters.attempts.saturating_add(1);
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                r => return r,
            }
        }
        Err(StreamStageError::Fenced)
    }
    async fn failed(
        &mut self,
        token: Uuid,
        error: StreamStageProcessError,
    ) -> Result<StreamStageResult, StreamStageError> {
        let permanent = matches!(error, StreamStageProcessError::Permanent(_));
        let reason = bounded_error(error.to_string());
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next.processing.as_mut().unwrap();
            m.lease = None;
            m.counters.failures = m.counters.failures.saturating_add(1);
            let r = m.retry.as_mut().expect("attempt persisted");
            r.last_error = Some(reason.clone());
            let result = if permanent || r.attempts >= self.config.max_attempts {
                r.terminal = true;
                r.next_attempt_at = None;
                StreamStageResult::Halted {
                    attempts: r.attempts,
                    reason: reason.clone(),
                }
            } else {
                let at = Utc::now()
                    + TimeDelta::milliseconds(
                        i64::try_from(self.config.backoff(r.attempts)).unwrap(),
                    );
                r.next_attempt_at = Some(at);
                m.counters.retries_scheduled = m.counters.retries_scheduled.saturating_add(1);
                StreamStageResult::RetryScheduled {
                    attempts: r.attempts,
                    next_attempt_at: at,
                }
            };
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {
                    continue;
                }
                r => r?,
            }
            return Ok(result);
        }
        Err(StreamStageError::Fenced)
    }
    /// Run one bounded batch. Cancelling/dropping this future cannot roll back a
    /// storage write or broker acknowledgement; replacement workers reload state.
    #[allow(clippy::too_many_lines)]
    pub async fn process_once<I, P, T>(
        &mut self,
        source: &mut T,
        processor: &P,
        cancel: &CancellationToken,
    ) -> Result<StreamStageResult, StreamStageError>
    where
        I: DeserializeOwned + Send + Sync,
        P: StreamStageProcessor<I, S, O>,
        T: StreamStageSource,
    {
        if source.identity() != self.source_identity {
            return Err(StreamStageError::DefinitionMismatch);
        }
        if cancel.is_cancelled() {
            return Ok(StreamStageResult::Cancelled {
                durable_generation: None,
            });
        }
        let token = match self.claim().await? {
            Ok(t) => t,
            Err(r) => return Ok(r),
        };
        let records = tokio::select! {
            biased;
            ()=cancel.cancelled()=>{self.release(token).await?;return Ok(StreamStageResult::Cancelled{durable_generation:None});},
            r=tokio::time::timeout(Duration::from_millis(self.config.receive_timeout_ms),source.receive(self.config.max_batch_records))=>match r {Ok(Ok(r))=>r,Ok(Err(e))=>{self.release(token).await?;return Err(e.into());},Err(_)=>{self.release(token).await?;return Ok(StreamStageResult::Idle);}}
        };
        if records.is_empty() {
            self.release(token).await?;
            return Ok(StreamStageResult::Idle);
        }
        validate_batch(&records, &self.config)?;
        let stored = self
            .checkpoint()
            .positions()
            .iter()
            .map(|p| (&p.lane, p.offset))
            .collect::<BTreeMap<_, _>>();
        let fresh = records
            .iter()
            .filter(|r| {
                stored
                    .get(&r.position.lane)
                    .is_none_or(|offset| r.position.offset > *offset)
            })
            .collect::<Vec<_>>();
        let recovered = records.len() - fresh.len();
        if let Some(retry) = &self.managed().retry
            && !fresh.is_empty()
            && !fresh.iter().any(|r| r.position == retry.anchor)
        {
            return Err(StreamStageError::RetryAnchorMissing);
        }
        let mut transition = None;
        let mut quarantined = Vec::new();
        let mut elapsed = 0;
        if !fresh.is_empty() {
            let anchor = self
                .managed()
                .retry
                .as_ref()
                .map_or_else(|| fresh[0].position.clone(), |r| r.anchor.clone());
            let mut inputs = Vec::new();
            for r in &fresh {
                let contract = self.config.input.contracts.get(&r.position.lane.source);
                if !self.validators.is_empty() && contract.is_none() {
                    self.release(token).await?;
                    return Err(StreamStageError::InvalidConfig(
                        "source has no pinned consume contract".into(),
                    ));
                }
                let violation = self
                    .validators
                    .get(&r.position.lane.source)
                    .and_then(|v| v.iter_errors(&r.message.payload).next())
                    .map(|e| {
                        (
                            StreamInputFailure::SchemaViolation,
                            bounded_error(format!(
                                "consume schema violation at schema {}",
                                e.schema_path()
                            )),
                        )
                    });
                let decoded = if let Some(failure) = violation {
                    Err(failure)
                } else {
                    serde_json::from_value(r.message.payload.clone()).map_err(|_| {
                        (
                            StreamInputFailure::TypedDecode,
                            "payload cannot be decoded into the processor input type".into(),
                        )
                    })
                };
                match decoded {
                    Ok(payload) => inputs.push(StreamStageInput {
                        position: r.position.clone(),
                        message: r.message.clone(),
                        payload,
                    }),
                    Err((failure, reason)) => {
                        if self.config.input.poison_policy == StreamPoisonPolicy::Halt {
                            self.begin_attempt(token, &r.position).await?;
                            return self
                                .failed(token, StreamStageProcessError::Permanent(reason))
                                .await;
                        }
                        quarantined.push(StreamQuarantinedInput {
                            id: Uuid::new_v4().to_string(),
                            position: r.position.clone(),
                            message: r.message.clone(),
                            failed_at: Utc::now(),
                            failure,
                            contract_sha256: contract.map(|c| c.sha256.clone()),
                            reason,
                        });
                    }
                }
            }
            let mut retained = self.managed().quarantine.clone();
            retained.extend(quarantined.clone());
            if !quarantine_fits(&retained, &self.config.input)? {
                self.release(token).await?;
                return Ok(StreamStageResult::Backpressured);
            }
            self.begin_attempt(token, &anchor).await?;
            let started = tokio::time::Instant::now();
            let result = tokio::select! {biased;()=cancel.cancelled()=>{self.release(token).await?;return Ok(StreamStageResult::Cancelled{durable_generation:None});},r=tokio::time::timeout(Duration::from_millis(self.config.processing_timeout_ms),async { if inputs.is_empty() { Ok(StreamStageTransition { state: self.checkpoint().state().clone(), outputs: Vec::new() }) } else { processor.process(self.checkpoint().state().clone(), &inputs).await } })=>match r {Ok(r)=>r,Err(_)=>Err(StreamStageProcessError::Retryable("processing timeout".into()))}};
            elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let result = match result {
                Ok(r) => r,
                Err(e) => return self.failed(token, e).await,
            };
            if result.outputs.len() > self.config.max_outputs_per_batch
                || serde_json::to_vec(&result.outputs)
                    .map_err(StreamCheckpointError::from)?
                    .len()
                    > self.config.max_output_bytes
            {
                return self
                    .failed(
                        token,
                        StreamStageProcessError::Permanent(
                            "processor exceeded output bounds".into(),
                        ),
                    )
                    .await;
            }
            transition = Some(result);
        }
        let positions = tokio::time::timeout(
            Duration::from_millis(self.config.source_timeout_ms),
            source.validate(&records),
        )
        .await
        .map_err(|_| StreamStageError::Timeout {
            operation: "validate",
        })??;
        if normalized(&positions)
            != normalized(
                &records
                    .iter()
                    .map(|r| r.position.clone())
                    .collect::<Vec<_>>(),
            )
        {
            return Err(StreamStageError::InvalidBatch(
                "validated positions do not match receipt batch".into(),
            ));
        }
        let revision = self.managed().revision;
        let mut durable = None;
        if let Some(t) = transition {
            for attempt in 0..CAS_RETRIES {
                self.reload().await?;
                self.owned(token)?;
                if self.managed().revision != revision {
                    return Err(StreamStageError::Fenced);
                }
                let mut next = self.coordinator.prepare_checkpoint(
                    t.state.clone(),
                    positions
                        .iter()
                        .filter(|p| {
                            self.checkpoint()
                                .positions()
                                .iter()
                                .find(|s| s.lane == p.lane)
                                .is_none_or(|s| p.offset >= s.offset)
                        })
                        .cloned(),
                    t.outputs.clone(),
                )?;
                let m = next.processing.as_mut().unwrap();
                m.quarantine.extend(quarantined.clone());
                if !quarantine_fits(&m.quarantine, &self.config.input)? {
                    self.release(token).await?;
                    return Ok(StreamStageResult::Backpressured);
                }
                m.counters.quarantined_records = m
                    .counters
                    .quarantined_records
                    .saturating_add(u64::try_from(quarantined.len()).unwrap_or(u64::MAX));
                m.revision = m
                    .revision
                    .checked_add(1)
                    .ok_or(StreamCheckpointError::GenerationOverflow)?;
                m.retry = None;
                m.counters.completed_batches = m.counters.completed_batches.saturating_add(1);
                m.counters.processed_records = m
                    .counters
                    .processed_records
                    .saturating_add(u64::try_from(fresh.len()).unwrap_or(u64::MAX));
                m.counters.recovered_records = m
                    .counters
                    .recovered_records
                    .saturating_add(u64::try_from(recovered).unwrap_or(u64::MAX));
                m.counters.total_processing_ms =
                    m.counters.total_processing_ms.saturating_add(elapsed);
                m.counters.last_processing_ms = elapsed;
                match self.persist(next).await {
                    Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict {
                        ..
                    })) if attempt + 1 < CAS_RETRIES => continue,
                    r => r?,
                }
                durable = Some(self.checkpoint().generation());
                break;
            }
        }
        if fresh.is_empty() && recovered > 0 {
            for attempt in 0..CAS_RETRIES {
                self.reload().await?;
                self.owned(token)?;
                let mut next = self.coordinator.snapshot.clone();
                let m = next.processing.as_mut().unwrap();
                m.counters.recovered_records = m
                    .counters
                    .recovered_records
                    .saturating_add(u64::try_from(recovered).unwrap_or(u64::MAX));
                match self.persist(next).await {
                    Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict {
                        ..
                    })) if attempt + 1 < CAS_RETRIES => {}
                    r => {
                        r?;
                        break;
                    }
                }
            }
        }
        if cancel.is_cancelled() {
            self.release(token).await?;
            return Ok(StreamStageResult::Cancelled {
                durable_generation: durable,
            });
        }
        self.owned(token)?;
        let generation = self.checkpoint().generation();
        let ack = tokio::time::timeout(
            Duration::from_millis(self.config.source_timeout_ms),
            source.acknowledge(&records),
        )
        .await
        .unwrap_or_else(|_| {
            Err(StreamStageSourceError::Retryable(
                "acknowledgement timeout; commit may have happened".into(),
            ))
        });
        if let Err(error) = ack {
            // Preserve the durable generation even if diagnostic bookkeeping is fenced.
            let _ = self.record_ack_failure(token).await;
            return Err(StreamStageError::Acknowledgement { generation, error });
        }
        self.release(token).await?;
        Ok(StreamStageResult::Completed {
            generation,
            processed: fresh.len(),
            recovered,
        })
    }
    async fn record_ack_failure(&mut self, token: Uuid) -> Result<(), StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            self.owned(token)?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next.processing.as_mut().unwrap();
            m.lease = None;
            m.counters.acknowledgement_failures =
                m.counters.acknowledgement_failures.saturating_add(1);
            match self.persist(next).await {
                Err(StreamStageError::Checkpoint(StreamCheckpointError::Conflict { .. })) => {}
                r => return r,
            }
        }
        Err(StreamStageError::Fenced)
    }
    /// Continuous driver. Source recovery creates new receipt capabilities;
    /// checkpointed positions are skipped rather than passed to the processor.
    pub async fn run<I, P, T>(
        &mut self,
        source: &mut T,
        processor: &P,
        cancel: &CancellationToken,
    ) -> Result<(), StreamStageError>
    where
        I: DeserializeOwned + Send + Sync,
        P: StreamStageProcessor<I, S, O>,
        T: StreamStageSource,
    {
        let result = self.run_inner(source, processor, cancel).await;
        let _ = tokio::time::timeout(
            Duration::from_millis(self.config.source_timeout_ms),
            source.close(),
        )
        .await;
        result
    }
    async fn run_inner<I, P, T>(
        &mut self,
        source: &mut T,
        processor: &P,
        cancel: &CancellationToken,
    ) -> Result<(), StreamStageError>
    where
        I: DeserializeOwned + Send + Sync,
        P: StreamStageProcessor<I, S, O>,
        T: StreamStageSource,
    {
        loop {
            let result = self.process_once(source, processor, cancel).await;
            let wait = match result {
                Ok(StreamStageResult::Cancelled { .. }) => {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(self.config.source_timeout_ms),
                        source.close(),
                    )
                    .await;
                    return Ok(());
                }
                Ok(StreamStageResult::Halted { attempts, reason }) => {
                    return Err(StreamStageError::InvalidBatch(format!(
                        "stage halted after {attempts} attempts: {reason}"
                    )));
                }
                Ok(StreamStageResult::RetryScheduled {
                    next_attempt_at, ..
                }) => {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(self.config.source_timeout_ms),
                        source.close(),
                    )
                    .await;
                    next_attempt_at
                        .signed_duration_since(Utc::now())
                        .to_std()
                        .unwrap_or_default()
                }
                Ok(StreamStageResult::Completed { .. }) => continue,
                Ok(StreamStageResult::Busy | StreamStageResult::Backpressured) => {
                    let _ = tokio::time::timeout(
                        Duration::from_millis(self.config.source_timeout_ms),
                        source.close(),
                    )
                    .await;
                    Duration::from_millis(self.config.poll_interval_ms)
                }
                Ok(_) => Duration::from_millis(self.config.poll_interval_ms),
                Err(
                    StreamStageError::Source(
                        StreamStageSourceError::Retryable(_) | StreamStageSourceError::Fenced(_),
                    )
                    | StreamStageError::Acknowledgement { .. },
                ) => {
                    tokio::time::timeout(
                        Duration::from_millis(self.config.source_timeout_ms),
                        source.recover(),
                    )
                    .await
                    .map_err(|_| StreamStageError::Timeout {
                        operation: "source recovery",
                    })??;
                    Duration::from_millis(self.config.poll_interval_ms)
                }
                Err(e) => return Err(e),
            };
            tokio::select! {()=cancel.cancelled()=>return Ok(()),()=tokio::time::sleep(wait)=>{}}
        }
    }
}
fn normalized(positions: &[StreamPosition]) -> BTreeMap<crate::StreamPositionLane, i64> {
    let mut m = BTreeMap::new();
    for p in positions {
        m.entry(p.lane.clone())
            .and_modify(|n: &mut i64| *n = (*n).max(p.offset))
            .or_insert(p.offset);
    }
    m
}
fn validate_batch<R>(
    records: &[StreamStageRecord<R>],
    config: &StreamStageConfig,
) -> Result<(), StreamStageError> {
    if records.len() > config.max_batch_records {
        return Err(StreamStageError::InvalidBatch(
            "record capacity exceeded".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut last = BTreeMap::new();
    let mut bytes = 0_usize;
    for r in records {
        let p = &r.position;
        if p.offset < 0
            || p.lane.partition < 0
            || p.lane.source.trim().is_empty()
            || p.lane.topic.trim().is_empty()
            || p.lane.consumer_group.trim().is_empty()
            || r.message.topic != p.lane.topic
            || r.message.partition != Some(p.lane.partition)
            || r.message.offset != Some(p.offset)
            || !seen.insert((p.lane.clone(), p.offset))
            || last
                .insert(p.lane.clone(), p.offset)
                .is_some_and(|n| n >= p.offset)
        {
            return Err(StreamStageError::InvalidBatch(
                "invalid, duplicated, or unordered source position".into(),
            ));
        }
        bytes = bytes.saturating_add(
            serde_json::to_vec(&r.message)
                .map_err(StreamCheckpointError::from)?
                .len(),
        );
        if bytes > config.max_batch_bytes {
            return Err(StreamStageError::InvalidBatch(
                "payload capacity exceeded".into(),
            ));
        }
    }
    Ok(())
}
fn bounded_error(mut error: String) -> String {
    let mut len = error.len().min(4096);
    while !error.is_char_boundary(len) {
        len -= 1;
    }
    error.truncate(len);
    error
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;

#[allow(clippy::trivially_copy_pass_by_ref)] // Serde skip predicates receive references.
fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn quarantine_fits(
    entries: &[StreamQuarantinedInput],
    policy: &StreamInputPolicy,
) -> Result<bool, StreamStageError> {
    Ok(entries.is_empty()
        || entries.len() <= policy.max_quarantined_records
            && serde_json::to_vec(entries)
                .map_err(StreamCheckpointError::from)?
                .len()
                <= policy.max_quarantine_bytes)
}

/// Durable operator access independent of Kafka consumers and processor state types.
pub struct StreamStageOperator {
    coordinator: StreamCheckpointCoordinator<serde_json::Value, serde_json::Value>,
}
impl StreamStageOperator {
    /// Missing checkpoints return None; unmanaged checkpoints are rejected.
    pub async fn load(
        store: std::sync::Arc<dyn acteon_state::StateStore>,
        key: acteon_state::StateKey,
    ) -> Result<Option<Self>, StreamStageError> {
        let Some(coordinator) = StreamCheckpointCoordinator::load_existing(store, key).await?
        else {
            return Ok(None);
        };
        if coordinator.snapshot.processing.is_none() {
            return Err(StreamStageError::MissingManagedState);
        }
        Ok(Some(Self { coordinator }))
    }
    /// A safe projection: no state, outputs, source identity, or lease capabilities.
    pub fn status(&self) -> StreamStageMetrics {
        let m = self.coordinator.snapshot.processing.as_ref().unwrap();
        StreamStageMetrics {
            counters: m.counters.clone(),
            checkpoint_generation: self.coordinator.snapshot.generation(),
            pending_outputs: self.coordinator.snapshot.pending_outputs().len(),
            quarantined_records: m.quarantine.len(),
            lease_expires_at: m.lease.as_ref().map(|l| l.expires_at),
            next_attempt_at: m.retry.as_ref().and_then(|r| r.next_attempt_at),
            halted: m.retry.as_ref().is_some_and(|r| r.terminal),
            last_error: m.retry.as_ref().and_then(|r| r.last_error.clone()),
        }
    }
    pub fn quarantined_inputs(&self) -> &[StreamQuarantinedInput] {
        &self
            .coordinator
            .snapshot
            .processing
            .as_ref()
            .unwrap()
            .quarantine
    }
    /// CAS retries preserve concurrent checkpoint and outbox updates. Idempotent.
    pub async fn discard(&mut self, id: &str) -> Result<bool, StreamStageError> {
        for _ in 0..CAS_RETRIES {
            self.coordinator.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next
                .processing
                .as_mut()
                .ok_or(StreamStageError::MissingManagedState)?;
            let Some(index) = m.quarantine.iter().position(|e| e.id == id) else {
                return Ok(false);
            };
            m.quarantine.remove(index);
            m.counters.discarded_quarantined_records = m
                .counters
                .discarded_quarantined_records
                .checked_add(1)
                .ok_or(StreamCheckpointError::GenerationOverflow)?;
            match self.coordinator.persist(next).await {
                Err(StreamCheckpointError::Conflict { .. }) => {}
                result => {
                    result?;
                    return Ok(true);
                }
            }
        }
        Err(StreamStageError::Fenced)
    }
}
