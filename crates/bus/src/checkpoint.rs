//! Durable stream checkpoints with an idempotent output outbox.
//!
//! This module stores processor state, consumed broker positions, and ready
//! outputs in one compare-and-swap record. A successful write returns the only
//! commit plan exposed by the API, making checkpoint-before-offset-commit the
//! natural control flow.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;

use acteon_core::{Namespace, TenantId};
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AcknowledgedSubscription, SubscriptionAck, SubscriptionError, SubscriptionReceipt};
use crate::{BusBackend, OffsetPosition};

const SNAPSHOT_VERSION: u16 = 5;
const STATE_KIND: &str = "bus_stream_checkpoint";
const DEFAULT_MAX_POSITIONS: usize = 10_000;
const DEFAULT_MAX_PENDING_OUTPUTS: usize = 100_000;

/// Build the state key used by a stream processor checkpoint.
#[must_use]
pub fn stream_checkpoint_key(
    namespace: impl Into<Namespace>,
    tenant: impl Into<TenantId>,
    processor_id: impl Into<String>,
) -> StateKey {
    StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(STATE_KIND.to_owned()),
        processor_id,
    )
}

/// A source, consumer group, topic, and partition with one ordered offset.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamPositionLane {
    pub source: String,
    pub consumer_group: String,
    pub topic: String,
    pub partition: i32,
}

/// The last fully represented source offset for one lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamPosition {
    pub lane: StreamPositionLane,
    pub offset: i64,
}

/// One pending downstream output, addressed by a stable idempotency key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOutboxEntry<T> {
    pub idempotency_key: String,
    /// When the output first entered the outbox, for backlog-age monitoring.
    pub created_at: DateTime<Utc>,
    pub payload: T,
}

/// Processing receipts from one live source session. Positions are derived
/// from these capabilities, never supplied as unchecked raw commit offsets.
pub struct SubscriptionCheckpointBatch<'a> {
    pub source: &'a str,
    pub subscription: &'a mut dyn AcknowledgedSubscription,
    pub receipts: &'a [SubscriptionReceipt],
}

/// Durable generation plus the broker-confirmed prefixes. Acknowledgements
/// may cover several partitions but never skip unprocessed earlier deliveries.
#[derive(Debug)]
pub struct AcknowledgedStreamCheckpoint<O> {
    pub checkpoint: PersistedStreamCheckpoint<O>,
    pub acknowledgements: Vec<SubscriptionAck>,
}

/// Hard limits persisted with a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamCheckpointConfig {
    pub max_positions: usize,
    pub max_pending_outputs: usize,
}

impl Default for StreamCheckpointConfig {
    fn default() -> Self {
        Self {
            max_positions: DEFAULT_MAX_POSITIONS,
            max_pending_outputs: DEFAULT_MAX_PENDING_OUTPUTS,
        }
    }
}

impl StreamCheckpointConfig {
    fn validate(&self) -> Result<(), StreamCheckpointError> {
        if self.max_positions == 0 {
            return Err(StreamCheckpointError::ZeroCapacity {
                field: "max_positions",
            });
        }
        if self.max_pending_outputs == 0 {
            return Err(StreamCheckpointError::ZeroCapacity {
                field: "max_pending_outputs",
            });
        }
        Ok(())
    }
}

/// Versioned recovery record stored atomically through [`StateStore`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    deny_unknown_fields,
    bound(deserialize = "S: Deserialize<'de>, O: Deserialize<'de>")
)]
pub struct StreamCheckpointSnapshot<S, O> {
    schema_version: u16,
    processor_id: String,
    pub(crate) generation: u64,
    config: StreamCheckpointConfig,
    state: S,
    positions: Vec<StreamPosition>,
    pub(crate) outbox: Vec<StreamOutboxEntry<O>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) managed: Option<crate::outbox::ManagedOutboxState<O>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) processing: Option<crate::stage::ManagedStageState>,
}

impl<S, O> StreamCheckpointSnapshot<S, O> {
    #[must_use]
    pub fn processor_id(&self) -> &str {
        &self.processor_id
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn config(&self) -> &StreamCheckpointConfig {
        &self.config
    }

    #[must_use]
    pub fn state(&self) -> &S {
        &self.state
    }

    #[must_use]
    pub fn positions(&self) -> &[StreamPosition] {
        &self.positions
    }

    #[must_use]
    pub fn pending_outputs(&self) -> &[StreamOutboxEntry<O>] {
        &self.outbox
    }
}

/// A commit and delivery plan that can only be created after durable storage.
#[derive(Debug, Clone, PartialEq)]
pub struct PersistedStreamCheckpoint<O> {
    generation: u64,
    positions: Vec<StreamPosition>,
    pending_outputs: Vec<StreamOutboxEntry<O>>,
}

impl<O> PersistedStreamCheckpoint<O> {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn positions(&self) -> &[StreamPosition] {
        &self.positions
    }

    #[must_use]
    pub fn pending_outputs(&self) -> &[StreamOutboxEntry<O>] {
        &self.pending_outputs
    }
}

/// Checkpoint protocol failures. Admission errors leave local and durable state
/// unchanged. A `Commit` error or an `Acknowledgement` error with a generation
/// happens after the named generation is durable.
#[derive(Debug, Error)]
pub enum StreamCheckpointError {
    #[error(
        "live acknowledgement failed for {signal_source} (durable generation: {generation:?}): {error}"
    )]
    Acknowledgement {
        /// None means validation failed before the checkpoint write.
        generation: Option<u64>,
        signal_source: String,
        #[source]
        error: SubscriptionError,
    },
    #[error(transparent)]
    State(#[from] StateError),
    #[error("checkpoint serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("checkpoint processor ID cannot be empty")]
    EmptyProcessorId,
    #[error("checkpoint state key must use kind '{STATE_KIND}'")]
    InvalidStateKey,
    #[error("{field} must be greater than zero")]
    ZeroCapacity { field: &'static str },
    #[error("unsupported stream checkpoint version {0}")]
    UnsupportedSnapshotVersion(u16),
    #[error("stream checkpoint belongs to processor '{actual}', expected '{expected}'")]
    ProcessorMismatch { expected: String, actual: String },
    #[error("stream checkpoint generation overflow")]
    GenerationOverflow,
    #[error("checkpoint contains duplicate {collection} entry")]
    DuplicateSnapshotEntry { collection: &'static str },
    #[error("checkpoint position has an empty {field}")]
    EmptyPositionField { field: &'static str },
    #[error("checkpoint offset cannot be negative")]
    NegativeOffset,
    #[error("checkpoint position capacity {limit} exceeded")]
    PositionCapacity { limit: usize },
    #[error("pending outbox capacity {limit} exceeded")]
    OutboxCapacity { limit: usize },
    #[error("outbox idempotency key cannot be empty")]
    EmptyIdempotencyKey,
    #[error("outbox idempotency key '{0}' is already pending")]
    DuplicateOutput(String),
    #[error(
        "offset regression for {signal_source}/{topic}/{partition}: stored {stored}, proposed {proposed}"
    )]
    PositionRegression {
        signal_source: String,
        topic: String,
        partition: i32,
        stored: i64,
        proposed: i64,
    },
    #[error("managed processing state must be changed through its stage lease")]
    ManagedProcessing,
    #[error("invalid managed stage snapshot: {0}")]
    InvalidManagedStage(String),
    #[error("managed output must be completed through its dispatcher lease")]
    ManagedOutput,
    #[error("invalid managed outbox snapshot: {0}")]
    InvalidManagedOutbox(String),
    #[error("outbox entry '{0}' is not pending")]
    UnknownOutput(String),
    #[error("checkpoint write conflicted: expected state version {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error(
        "offset commit failed after checkpoint generation {generation} became durable for {signal_source}/{topic}/{partition}: {message}"
    )]
    Commit {
        generation: u64,
        signal_source: String,
        topic: String,
        partition: i32,
        message: String,
    },
}

/// Coordinates durable stream state and outbox transitions through Acteon's
/// optimistic-concurrency state-store contract.
pub struct StreamCheckpointCoordinator<S, O> {
    store: Arc<dyn StateStore>,
    key: StateKey,
    store_version: u64,
    pub(crate) snapshot: StreamCheckpointSnapshot<S, O>,
}

impl<S, O> StreamCheckpointCoordinator<S, O>
where
    S: Clone + Serialize + DeserializeOwned + Send + Sync,
    O: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    /// Load an existing checkpoint, or atomically create generation zero.
    pub async fn initialize(
        store: Arc<dyn StateStore>,
        key: StateKey,
        initial_state: S,
        config: StreamCheckpointConfig,
    ) -> Result<Self, StreamCheckpointError> {
        validate_key(&key)?;
        config.validate()?;
        if let Some((value, version)) = store.get_versioned(&key).await? {
            return Self::decode(store, key, version, &value);
        }

        let snapshot = StreamCheckpointSnapshot {
            schema_version: 1,
            processor_id: key.id.clone(),
            generation: 0,
            config,
            state: initial_state,
            positions: Vec::new(),
            outbox: Vec::new(),
            managed: None,
            processing: None,
        };
        let encoded = serde_json::to_string(&snapshot)?;
        if store.check_and_set(&key, &encoded, None).await? {
            return Ok(Self {
                store,
                key,
                store_version: 1,
                snapshot,
            });
        }
        let (value, version) = store
            .get_versioned(&key)
            .await?
            .ok_or_else(|| StateError::NotFound(key.to_string()))?;
        Self::decode(store, key, version, &value)
    }

    fn decode(
        store: Arc<dyn StateStore>,
        key: StateKey,
        store_version: u64,
        encoded: &str,
    ) -> Result<Self, StreamCheckpointError> {
        let snapshot: StreamCheckpointSnapshot<S, O> = serde_json::from_str(encoded)?;
        validate_snapshot(&snapshot, &key.id)?;
        Ok(Self {
            store,
            key,
            store_version,
            snapshot,
        })
    }

    /// Reload durable state, discarding this instance's unpersisted view.
    pub async fn reload(&mut self) -> Result<(), StreamCheckpointError> {
        let (value, version) = self
            .store
            .get_versioned(&self.key)
            .await?
            .ok_or_else(|| StateError::NotFound(self.key.to_string()))?;
        let decoded = Self::decode(Arc::clone(&self.store), self.key.clone(), version, &value)?;
        self.store_version = decoded.store_version;
        self.snapshot = decoded.snapshot;
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> &StreamCheckpointSnapshot<S, O> {
        &self.snapshot
    }

    /// Load an existing checkpoint without creating or modifying it.
    pub async fn load_existing(
        store: Arc<dyn StateStore>,
        key: StateKey,
    ) -> Result<Option<Self>, StreamCheckpointError> {
        validate_key(&key)?;
        match store.get_versioned(&key).await? {
            Some((value, version)) => Self::decode(store, key, version, &value).map(Some),
            None => Ok(None),
        }
    }

    /// Current durable commit and delivery plan. This is useful for retrying
    /// broker commits after [`StreamCheckpointError::Commit`].
    #[must_use]
    pub fn persisted(&self) -> PersistedStreamCheckpoint<O> {
        self.persisted_view()
    }

    /// Atomically persist processor state, source positions, and new outputs.
    pub async fn checkpoint<PI, OI>(
        &mut self,
        state: S,
        positions: PI,
        outputs: OI,
    ) -> Result<PersistedStreamCheckpoint<O>, StreamCheckpointError>
    where
        PI: IntoIterator<Item = StreamPosition>,
        OI: IntoIterator<Item = StreamOutboxEntry<O>>,
    {
        if self.snapshot.processing.is_some() {
            return Err(StreamCheckpointError::ManagedProcessing);
        }
        let next = self.prepare_checkpoint(state, positions, outputs)?;
        self.persist(next).await?;
        Ok(self.persisted_view())
    }

    /// Persist first, then commit every returned broker position in stable
    /// order. A commit failure never rolls back the durable checkpoint.
    pub async fn checkpoint_then_commit<PI, OI, F, Fut>(
        &mut self,
        state: S,
        positions: PI,
        outputs: OI,
        mut commit: F,
    ) -> Result<PersistedStreamCheckpoint<O>, StreamCheckpointError>
    where
        PI: IntoIterator<Item = StreamPosition>,
        OI: IntoIterator<Item = StreamOutboxEntry<O>>,
        F: FnMut(StreamPosition) -> Fut,
        Fut: Future<Output = Result<(), String>>,
    {
        let persisted = self.checkpoint(state, positions, outputs).await?;
        for position in &persisted.positions {
            commit(position.clone())
                .await
                .map_err(|message| StreamCheckpointError::Commit {
                    generation: persisted.generation,
                    signal_source: position.lane.source.clone(),
                    topic: position.lane.topic.clone(),
                    partition: position.lane.partition,
                    message,
                })?;
        }
        Ok(persisted)
    }

    /// Validate live receipt ownership and processing prefixes, persist state
    /// and outputs, then acknowledge through the original consumer sessions.
    ///
    /// A rebalance/commit failure after persistence leaves that generation and
    /// its outbox durable. Kafka redelivery must recover from the checkpoint;
    /// this is not an atomic transaction between the state store and Kafka.
    pub async fn checkpoint_then_acknowledge<OI>(
        &mut self,
        state: S,
        outputs: OI,
        batches: &mut [SubscriptionCheckpointBatch<'_>],
    ) -> Result<AcknowledgedStreamCheckpoint<O>, StreamCheckpointError>
    where
        OI: IntoIterator<Item = StreamOutboxEntry<O>>,
    {
        let mut positions = Vec::new();
        for batch in batches.iter() {
            if batch.source.trim().is_empty() {
                return Err(StreamCheckpointError::EmptyPositionField { field: "source" });
            }
            batch
                .subscription
                .validate_checkpoint_receipts(batch.receipts)
                .map_err(|error| StreamCheckpointError::Acknowledgement {
                    generation: None,
                    signal_source: batch.source.into(),
                    error,
                })?;
            positions.extend(batch.receipts.iter().map(|receipt| StreamPosition {
                lane: StreamPositionLane {
                    source: batch.source.into(),
                    consumer_group: receipt.consumer_group().into(),
                    topic: receipt.topic().into(),
                    partition: receipt.position().partition,
                },
                offset: receipt.position().offset,
            }));
        }
        let checkpoint = self.checkpoint(state, positions, outputs).await?;
        let mut acknowledgements = Vec::new();
        for batch in batches {
            let ack = batch
                .subscription
                .acknowledge(batch.receipts)
                .await
                .map_err(|error| StreamCheckpointError::Acknowledgement {
                    generation: Some(checkpoint.generation),
                    signal_source: batch.source.into(),
                    error,
                })?;
            acknowledgements.push(ack);
        }
        Ok(AcknowledgedStreamCheckpoint {
            checkpoint,
            acknowledgements,
        })
    }

    /// Persist a checkpoint and commit its positions through an Acteon bus
    /// backend. This is the direct counterpart to [`Self::checkpoint_then_commit`]
    /// for consumers that do not need a custom commit adapter. This legacy
    /// raw-position path does not fence active assignment ownership. Prefer
    /// [`Self::checkpoint_then_acknowledge`] for live Kafka processing.
    pub async fn checkpoint_then_commit_bus<PI, OI>(
        &mut self,
        state: S,
        positions: PI,
        outputs: OI,
        backend: &dyn BusBackend,
    ) -> Result<PersistedStreamCheckpoint<O>, StreamCheckpointError>
    where
        PI: IntoIterator<Item = StreamPosition>,
        OI: IntoIterator<Item = StreamOutboxEntry<O>>,
    {
        let persisted = self.checkpoint(state, positions, outputs).await?;
        for position in &persisted.positions {
            backend
                .commit_offset(
                    &position.lane.topic,
                    &position.lane.consumer_group,
                    OffsetPosition {
                        partition: position.lane.partition,
                        offset: position.offset,
                    },
                )
                .await
                .map_err(|error| StreamCheckpointError::Commit {
                    generation: persisted.generation,
                    signal_source: position.lane.source.clone(),
                    topic: position.lane.topic.clone(),
                    partition: position.lane.partition,
                    message: error.to_string(),
                })?;
        }
        Ok(persisted)
    }

    /// Remove successfully delivered idempotency keys in a new durable
    /// generation. Redelivery before this succeeds is expected and safe.
    pub async fn acknowledge_outputs<I, T>(
        &mut self,
        delivered: I,
    ) -> Result<PersistedStreamCheckpoint<O>, StreamCheckpointError>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let delivered = delivered
            .into_iter()
            .map(|key| key.as_ref().to_owned())
            .collect::<BTreeSet<_>>();
        if delivered.is_empty() {
            return Ok(self.persisted_view());
        }
        if self.snapshot.managed.is_some() {
            return Err(StreamCheckpointError::ManagedOutput);
        }
        let pending = self
            .snapshot
            .outbox
            .iter()
            .map(|entry| entry.idempotency_key.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(unknown) = delivered.iter().find(|key| !pending.contains(key.as_str())) {
            return Err(StreamCheckpointError::UnknownOutput(unknown.clone()));
        }
        let mut next = self.snapshot.clone();
        next.generation = next
            .generation
            .checked_add(1)
            .ok_or(StreamCheckpointError::GenerationOverflow)?;
        next.outbox
            .retain(|entry| !delivered.contains(&entry.idempotency_key));
        self.persist(next).await?;
        Ok(self.persisted_view())
    }

    pub(crate) fn prepare_checkpoint<PI, OI>(
        &self,
        state: S,
        positions: PI,
        outputs: OI,
    ) -> Result<StreamCheckpointSnapshot<S, O>, StreamCheckpointError>
    where
        PI: IntoIterator<Item = StreamPosition>,
        OI: IntoIterator<Item = StreamOutboxEntry<O>>,
    {
        let mut position_map = positions_to_map(&self.snapshot.positions)?;
        let mut proposed_positions = BTreeMap::new();
        for position in positions {
            validate_position(&position)?;
            proposed_positions
                .entry(position.lane)
                .and_modify(|offset: &mut i64| *offset = (*offset).max(position.offset))
                .or_insert(position.offset);
        }
        for (lane, proposed) in proposed_positions {
            if let Some(stored) = position_map.get(&lane)
                && proposed < *stored
            {
                return Err(StreamCheckpointError::PositionRegression {
                    signal_source: lane.source,
                    topic: lane.topic,
                    partition: lane.partition,
                    stored: *stored,
                    proposed,
                });
            }
            position_map.insert(lane, proposed);
        }
        if position_map.len() > self.snapshot.config.max_positions {
            return Err(StreamCheckpointError::PositionCapacity {
                limit: self.snapshot.config.max_positions,
            });
        }

        let mut outbox = self
            .snapshot
            .outbox
            .iter()
            .cloned()
            .map(|entry| (entry.idempotency_key.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        for entry in outputs {
            validate_output(&entry)?;
            if outbox.contains_key(&entry.idempotency_key)
                || self
                    .snapshot
                    .managed
                    .as_ref()
                    .is_some_and(|managed| managed.contains_dead_letter(&entry.idempotency_key))
            {
                return Err(StreamCheckpointError::DuplicateOutput(
                    entry.idempotency_key,
                ));
            }
            outbox.insert(entry.idempotency_key.clone(), entry);
        }
        if outbox.len() > self.snapshot.config.max_pending_outputs {
            return Err(StreamCheckpointError::OutboxCapacity {
                limit: self.snapshot.config.max_pending_outputs,
            });
        }

        Ok(StreamCheckpointSnapshot {
            schema_version: self.snapshot.schema_version,
            processor_id: self.snapshot.processor_id.clone(),
            generation: self
                .snapshot
                .generation
                .checked_add(1)
                .ok_or(StreamCheckpointError::GenerationOverflow)?,
            config: self.snapshot.config.clone(),
            state,
            positions: position_map
                .into_iter()
                .map(|(lane, offset)| StreamPosition { lane, offset })
                .collect(),
            outbox: outbox.into_values().collect(),
            managed: self.snapshot.managed.clone(),
            processing: self.snapshot.processing.clone(),
        })
    }

    pub(crate) async fn persist(
        &mut self,
        mut next: StreamCheckpointSnapshot<S, O>,
    ) -> Result<(), StreamCheckpointError> {
        next.schema_version = if next.processing.is_some() {
            if next
                .processing
                .as_ref()
                .is_some_and(crate::stage::ManagedStageState::requires_v5)
            {
                5
            } else if next
                .processing
                .as_ref()
                .is_some_and(crate::stage::ManagedStageState::requires_v4)
            {
                4
            } else {
                3
            }
        } else if next.managed.is_some() {
            2
        } else {
            1
        };
        validate_snapshot(&next, &self.key.id)?;
        let next_store_version = self
            .store_version
            .checked_add(1)
            .ok_or(StreamCheckpointError::GenerationOverflow)?;
        let encoded = serde_json::to_string(&next)?;
        match self
            .store
            .compare_and_swap(&self.key, self.store_version, &encoded, None)
            .await?
        {
            CasResult::Ok => {
                self.store_version = next_store_version;
                self.snapshot = next;
                Ok(())
            }
            CasResult::Conflict {
                current_version, ..
            } => Err(StreamCheckpointError::Conflict {
                expected: self.store_version,
                actual: current_version,
            }),
        }
    }

    fn persisted_view(&self) -> PersistedStreamCheckpoint<O> {
        PersistedStreamCheckpoint {
            generation: self.snapshot.generation,
            positions: self.snapshot.positions.clone(),
            pending_outputs: self.snapshot.outbox.clone(),
        }
    }
}

fn validate_key(key: &StateKey) -> Result<(), StreamCheckpointError> {
    if key.id.trim().is_empty() {
        return Err(StreamCheckpointError::EmptyProcessorId);
    }
    if key.kind.as_str() != STATE_KIND {
        return Err(StreamCheckpointError::InvalidStateKey);
    }
    Ok(())
}

fn validate_snapshot<S, O>(
    snapshot: &StreamCheckpointSnapshot<S, O>,
    expected_processor_id: &str,
) -> Result<(), StreamCheckpointError> {
    if !(1..=SNAPSHOT_VERSION).contains(&snapshot.schema_version) {
        return Err(StreamCheckpointError::UnsupportedSnapshotVersion(
            snapshot.schema_version,
        ));
    }
    if snapshot.processor_id != expected_processor_id {
        return Err(StreamCheckpointError::ProcessorMismatch {
            expected: expected_processor_id.to_owned(),
            actual: snapshot.processor_id.clone(),
        });
    }
    snapshot.config.validate()?;
    if snapshot.positions.len() > snapshot.config.max_positions {
        return Err(StreamCheckpointError::PositionCapacity {
            limit: snapshot.config.max_positions,
        });
    }
    if snapshot.schema_version == 1 && snapshot.managed.is_some() {
        return Err(StreamCheckpointError::InvalidManagedOutbox(
            "managed state requires version 2".into(),
        ));
    }
    if snapshot.processing.is_some() && snapshot.schema_version < 3 {
        return Err(StreamCheckpointError::InvalidManagedStage(
            "managed processing requires version 3".into(),
        ));
    }
    if let Some(stage) = &snapshot.processing {
        if stage.requires_v5() && snapshot.schema_version < 5 {
            return Err(StreamCheckpointError::InvalidManagedStage(
                "replay audit requires version 5".into(),
            ));
        }
        if stage.requires_v4() && snapshot.schema_version < 4 {
            return Err(StreamCheckpointError::InvalidManagedStage(
                "consume contracts/quarantine require version 4".into(),
            ));
        }
        stage
            .validate_positions(&snapshot.positions, snapshot.generation)
            .map_err(StreamCheckpointError::InvalidManagedStage)?;
        stage
            .validate()
            .map_err(StreamCheckpointError::InvalidManagedStage)?;
    }
    positions_to_map(&snapshot.positions)?;
    if snapshot.outbox.len() > snapshot.config.max_pending_outputs {
        return Err(StreamCheckpointError::OutboxCapacity {
            limit: snapshot.config.max_pending_outputs,
        });
    }
    let mut output_keys = BTreeSet::new();
    for output in &snapshot.outbox {
        validate_output(output)?;
        if !output_keys.insert(&output.idempotency_key) {
            return Err(StreamCheckpointError::DuplicateSnapshotEntry {
                collection: "outbox",
            });
        }
    }
    if let Some(managed) = &snapshot.managed {
        managed
            .validate(&output_keys)
            .map_err(StreamCheckpointError::InvalidManagedOutbox)?;
    }
    Ok(())
}

fn positions_to_map(
    positions: &[StreamPosition],
) -> Result<BTreeMap<StreamPositionLane, i64>, StreamCheckpointError> {
    let mut mapped = BTreeMap::new();
    for position in positions {
        validate_position(position)?;
        if mapped
            .insert(position.lane.clone(), position.offset)
            .is_some()
        {
            return Err(StreamCheckpointError::DuplicateSnapshotEntry {
                collection: "position",
            });
        }
    }
    Ok(mapped)
}

fn validate_position(position: &StreamPosition) -> Result<(), StreamCheckpointError> {
    for (field, value) in [
        ("source", position.lane.source.as_str()),
        ("consumer_group", position.lane.consumer_group.as_str()),
        ("topic", position.lane.topic.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StreamCheckpointError::EmptyPositionField { field });
        }
    }
    if position.offset < 0 {
        return Err(StreamCheckpointError::NegativeOffset);
    }
    Ok(())
}

fn validate_output<T>(output: &StreamOutboxEntry<T>) -> Result<(), StreamCheckpointError> {
    if output.idempotency_key.trim().is_empty() {
        return Err(StreamCheckpointError::EmptyIdempotencyKey);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use acteon_core::Topic;
    use acteon_state_memory::MemoryStateStore;
    use serde_json::{Value, json};

    use super::*;
    use crate::MemoryBackend;

    struct TestSubscription {
        ledger: crate::subscription::ReceiptLedger,
        store: Arc<dyn StateStore>,
        fail_ack: bool,
        ack_calls: usize,
    }

    impl TestSubscription {
        fn new(store: Arc<dyn StateStore>) -> Self {
            let mut ledger = crate::subscription::ReceiptLedger::new(
                "observability.acme.metrics".into(),
                "detector-metrics".into(),
                &crate::SubscriptionConfig::default(),
            );
            ledger.transition([0], false);
            Self {
                ledger,
                store,
                fail_ack: false,
                ack_calls: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl AcknowledgedSubscription for TestSubscription {
        async fn recv(&mut self) -> Result<crate::SubscriptionDelivery, SubscriptionError> {
            Err(SubscriptionError::Unsupported)
        }
        fn validate_receipts(
            &self,
            receipts: &[SubscriptionReceipt],
        ) -> Result<(), SubscriptionError> {
            self.ledger.validate(receipts)
        }
        fn validate_checkpoint_receipts(
            &self,
            receipts: &[SubscriptionReceipt],
        ) -> Result<(), SubscriptionError> {
            self.ledger.validate_checkpoint(receipts)
        }
        async fn acknowledge(
            &mut self,
            receipts: &[SubscriptionReceipt],
        ) -> Result<SubscriptionAck, SubscriptionError> {
            self.ack_calls += 1;
            let raw = self.store.get(&key()).await.unwrap().unwrap();
            let persisted: StreamCheckpointSnapshot<Value, Value> =
                serde_json::from_str(&raw).unwrap();
            assert_eq!(
                persisted.generation(),
                1,
                "checkpoint must already be durable"
            );
            assert_eq!(persisted.pending_outputs().len(), 1);
            assert_eq!(
                persisted.positions()[0].offset,
                receipts.last().unwrap().position().offset
            );
            if self.fail_ack {
                self.ledger.transition([], false);
                return Err(SubscriptionError::StaleReceipt);
            }
            let (epoch, commits) = self.ledger.prepare_ack(receipts)?;
            self.ledger.finish_ack(epoch, commits)
        }
        fn ownership_changes(&self) -> tokio::sync::watch::Receiver<crate::SubscriptionOwnership> {
            self.ledger.watch()
        }
    }

    #[tokio::test]
    async fn live_checkpoint_derives_offsets_and_persists_before_acknowledgement() {
        let state = store();
        let mut coordinator = coordinator(Arc::clone(&state)).await;
        let mut subscription = TestSubscription::new(state);
        let receipts = [
            subscription.ledger.deliver(0, 2).unwrap(),
            subscription.ledger.deliver(0, 5).unwrap(),
        ];
        let result = coordinator
            .checkpoint_then_acknowledge(
                json!({"seq": 1}),
                [output("ready")],
                &mut [SubscriptionCheckpointBatch {
                    source: "metrics",
                    subscription: &mut subscription,
                    receipts: &receipts,
                }],
            )
            .await
            .unwrap();
        assert_eq!(result.checkpoint.positions()[0].offset, 5);
        assert_eq!(result.acknowledgements[0].committed[0].offset, 5);
        assert_eq!(result.acknowledgements[0].remaining_in_flight, 0);
    }

    #[tokio::test]
    async fn live_checkpoint_rejects_gaps_and_revoked_receipts_before_persistence() {
        let state = store();
        let mut coordinator = coordinator(Arc::clone(&state)).await;
        let mut subscription = TestSubscription::new(state);
        subscription.ledger.deliver(0, 0).unwrap();
        let receipts = [subscription.ledger.deliver(0, 1).unwrap()];
        let error = coordinator
            .checkpoint_then_acknowledge(
                json!({"seq": 1}),
                [output("ready")],
                &mut [SubscriptionCheckpointBatch {
                    source: "metrics",
                    subscription: &mut subscription,
                    receipts: &receipts,
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamCheckpointError::Acknowledgement {
                generation: None,
                error: SubscriptionError::CheckpointGap,
                ..
            }
        ));
        assert_eq!(coordinator.snapshot().generation(), 0);
        subscription.ledger.transition([0], false);
        let error = coordinator
            .checkpoint_then_acknowledge(
                json!({"seq": 1}),
                [output("ready")],
                &mut [SubscriptionCheckpointBatch {
                    source: "metrics",
                    subscription: &mut subscription,
                    receipts: &receipts,
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamCheckpointError::Acknowledgement {
                generation: None,
                error: SubscriptionError::StaleReceipt,
                ..
            }
        ));
        assert_eq!(coordinator.snapshot().generation(), 0);
    }

    #[tokio::test]
    async fn revocation_after_checkpoint_preserves_recoverable_outputs() {
        let state = store();
        let mut coordinator = coordinator(Arc::clone(&state)).await;
        let mut subscription = TestSubscription::new(Arc::clone(&state));
        subscription.fail_ack = true;
        let receipts = [subscription.ledger.deliver(0, 1).unwrap()];
        let error = coordinator
            .checkpoint_then_acknowledge(
                json!({"seq": 1}),
                [output("ready")],
                &mut [SubscriptionCheckpointBatch {
                    source: "metrics",
                    subscription: &mut subscription,
                    receipts: &receipts,
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamCheckpointError::Acknowledgement {
                generation: Some(1),
                error: SubscriptionError::StaleReceipt,
                ..
            }
        ));
        let recovered = super::tests::coordinator(state).await;
        assert_eq!(recovered.snapshot().generation(), 1);
        assert_eq!(
            recovered.snapshot().pending_outputs()[0].idempotency_key,
            "ready"
        );
        assert_eq!(recovered.snapshot().positions()[0].offset, 1);
    }

    #[tokio::test]
    async fn checkpoint_cas_conflict_never_acknowledges_live_receipts() {
        let state = store();
        let mut stale = coordinator(Arc::clone(&state)).await;
        let mut winner = coordinator(Arc::clone(&state)).await;
        let mut subscription = TestSubscription::new(Arc::clone(&state));
        let receipts = [subscription.ledger.deliver(0, 1).unwrap()];
        winner
            .checkpoint(json!({"winner":true}), [position(1)], [output("winner")])
            .await
            .unwrap();
        let error = stale
            .checkpoint_then_acknowledge(
                json!({"stale":true}),
                [output("stale")],
                &mut [SubscriptionCheckpointBatch {
                    source: "metrics",
                    subscription: &mut subscription,
                    receipts: &receipts,
                }],
            )
            .await
            .unwrap_err();
        assert!(matches!(error, StreamCheckpointError::Conflict { .. }));
        assert_eq!(subscription.ack_calls, 0);
        let recovered = coordinator(state).await;
        assert_eq!(
            recovered.snapshot().pending_outputs()[0].idempotency_key,
            "winner"
        );
    }

    fn store() -> Arc<dyn StateStore> {
        Arc::new(MemoryStateStore::new())
    }

    fn key() -> StateKey {
        stream_checkpoint_key("observability", "acme", "neural-detector")
    }

    fn position(offset: i64) -> StreamPosition {
        StreamPosition {
            lane: StreamPositionLane {
                source: "metrics".to_owned(),
                consumer_group: "detector-metrics".to_owned(),
                topic: "observability.acme.metrics".to_owned(),
                partition: 0,
            },
            offset,
        }
    }

    fn output(id: &str) -> StreamOutboxEntry<Value> {
        StreamOutboxEntry {
            idempotency_key: id.to_owned(),
            created_at: "2026-10-02T19:42:00Z".parse().unwrap(),
            payload: json!({"incident": id}),
        }
    }

    async fn coordinator(state: Arc<dyn StateStore>) -> StreamCheckpointCoordinator<Value, Value> {
        StreamCheckpointCoordinator::initialize(
            state,
            key(),
            json!({"windows": []}),
            StreamCheckpointConfig::default(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn checkpoint_is_durable_before_offset_commit() {
        let state = store();
        let mut coordinator = coordinator(Arc::clone(&state)).await;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let callback_observed = Arc::clone(&observed);
        let callback_store = Arc::clone(&state);

        let persisted = coordinator
            .checkpoint_then_commit(
                json!({"windows": ["partial"]}),
                [position(7)],
                [output("incident-1")],
                move |position| {
                    let observed = Arc::clone(&callback_observed);
                    let store = Arc::clone(&callback_store);
                    async move {
                        let (encoded, _) = store
                            .get_versioned(&key())
                            .await
                            .map_err(|error| error.to_string())?
                            .ok_or_else(|| "missing checkpoint".to_owned())?;
                        let snapshot: StreamCheckpointSnapshot<Value, Value> =
                            serde_json::from_str(&encoded).map_err(|error| error.to_string())?;
                        observed
                            .lock()
                            .unwrap()
                            .push((snapshot.generation(), position.offset));
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();

        assert_eq!(persisted.generation(), 1);
        assert_eq!(*observed.lock().unwrap(), vec![(1, 7)]);
    }

    #[tokio::test]
    async fn outbox_survives_reload_until_acknowledged() {
        let state = store();
        let mut first = coordinator(Arc::clone(&state)).await;
        first
            .checkpoint(json!({"seq": 1}), [position(3)], [output("incident-1")])
            .await
            .unwrap();

        let mut recovered = coordinator(Arc::clone(&state)).await;
        assert_eq!(recovered.snapshot().generation(), 1);
        assert_eq!(recovered.snapshot().pending_outputs().len(), 1);
        let persisted = recovered.acknowledge_outputs(["incident-1"]).await.unwrap();
        assert_eq!(persisted.generation(), 2);
        assert_eq!(persisted.pending_outputs().len(), 0);

        let recovered = coordinator(state).await;
        assert_eq!(recovered.snapshot().pending_outputs().len(), 0);
    }

    #[tokio::test]
    async fn stale_writer_conflicts_without_overwriting_the_winner() {
        let state = store();
        let mut winner = coordinator(Arc::clone(&state)).await;
        let mut loser = coordinator(Arc::clone(&state)).await;
        winner
            .checkpoint(
                json!({"writer": "winner"}),
                [position(2)],
                [output("winner")],
            )
            .await
            .unwrap();

        let error = loser
            .checkpoint(json!({"writer": "stale"}), [position(3)], [output("stale")])
            .await
            .unwrap_err();
        assert!(matches!(error, StreamCheckpointError::Conflict { .. }));
        loser.reload().await.unwrap();
        assert_eq!(loser.snapshot().state(), &json!({"writer": "winner"}));
    }

    #[tokio::test]
    async fn failed_commit_leaves_the_checkpoint_and_outbox_durable() {
        let state = store();
        let mut active = coordinator(Arc::clone(&state)).await;
        let error = active
            .checkpoint_then_commit(
                json!({"seq": 1}),
                [position(9)],
                [output("incident-9")],
                |_| async { Err("broker unavailable".to_owned()) },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StreamCheckpointError::Commit { generation: 1, .. }
        ));

        let recovered = coordinator(state).await;
        assert_eq!(recovered.snapshot().positions()[0].offset, 9);
        assert_eq!(
            recovered.snapshot().pending_outputs()[0].idempotency_key,
            "incident-9"
        );
    }

    #[tokio::test]
    async fn admission_failures_do_not_advance_generation() {
        let state = store();
        let mut coordinator = coordinator(state).await;
        coordinator
            .checkpoint(json!({"seq": 1}), [position(5)], [output("incident-1")])
            .await
            .unwrap();

        let regression = coordinator
            .checkpoint(json!({"seq": 2}), [position(4)], Vec::new())
            .await
            .unwrap_err();
        assert!(matches!(
            regression,
            StreamCheckpointError::PositionRegression { .. }
        ));
        let duplicate = coordinator
            .checkpoint(json!({"seq": 2}), [position(6)], [output("incident-1")])
            .await
            .unwrap_err();
        assert!(matches!(
            duplicate,
            StreamCheckpointError::DuplicateOutput(_)
        ));
        assert_eq!(coordinator.snapshot().generation(), 1);
        assert_eq!(coordinator.snapshot().positions()[0].offset, 5);
    }

    #[tokio::test]
    async fn restore_rejects_duplicate_positions_in_a_corrupted_snapshot() {
        let state = store();
        let active = coordinator(Arc::clone(&state)).await;
        let mut corrupted = active.snapshot().clone();
        corrupted.positions = vec![position(1), position(2)];
        state
            .set(&key(), &serde_json::to_string(&corrupted).unwrap(), None)
            .await
            .unwrap();

        let result = StreamCheckpointCoordinator::<Value, Value>::initialize(
            state,
            key(),
            json!({}),
            StreamCheckpointConfig::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(StreamCheckpointError::DuplicateSnapshotEntry {
                collection: "position"
            })
        ));
    }

    #[tokio::test]
    async fn a_batch_uses_each_partitions_maximum_offset_independent_of_order() {
        let state = store();
        let mut coordinator = coordinator(state).await;
        let persisted = coordinator
            .checkpoint(
                json!({"seq": 1}),
                [position(7), position(5), position(6)],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(persisted.positions()[0].offset, 7);
    }

    #[tokio::test]
    async fn bus_commit_helper_advances_the_backend_only_after_persistence() {
        let state = store();
        let mut coordinator = coordinator(state).await;
        let backend = MemoryBackend::new();
        let topic = Topic::new("metrics", "observability", "acme");
        backend.create_topic(&topic).await.unwrap();

        coordinator
            .checkpoint_then_commit_bus(
                json!({"seq": 1}),
                [position(11)],
                [output("incident-11")],
                backend.as_ref(),
            )
            .await
            .unwrap();

        let lag = backend
            .consumer_lag(&topic.kafka_topic_name(), "detector-metrics")
            .await
            .unwrap();
        assert_eq!(lag[0].committed, 11);
    }
}
