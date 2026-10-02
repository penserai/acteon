//! Leased, at-least-once delivery of checkpoint outbox entries.
//!
//! Claims, retry schedules, completion, and dead letters share the checkpoint's
//! CAS record. Receivers must deduplicate by `idempotency_key`: lease fencing
//! protects durable state, not side effects already accepted by a receiver.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    BusError, BusMessage, SharedBackend, StreamCheckpointCoordinator, StreamCheckpointError,
    StreamOutboxEntry,
};

const CAS_RETRIES: usize = 8;

/// Delivery policy, persisted once and required to match across workers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOutboxConfig {
    pub lease_ms: u64,
    pub delivery_timeout_ms: u64,
    /// Includes attempts interrupted by a worker crash or cancellation.
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub max_dead_letters: usize,
}

impl Default for StreamOutboxConfig {
    fn default() -> Self {
        Self {
            lease_ms: 30_000,
            delivery_timeout_ms: 20_000,
            max_attempts: 5,
            initial_backoff_ms: 1_000,
            max_backoff_ms: 60_000,
            max_dead_letters: 10_000,
        }
    }
}

impl StreamOutboxConfig {
    fn validate(&self) -> Result<(), String> {
        if self.delivery_timeout_ms == 0
            || self.lease_ms <= self.delivery_timeout_ms
            || self.max_attempts == 0
            || self.initial_backoff_ms == 0
            || self.max_backoff_ms < self.initial_backoff_ms
            || self.max_dead_letters == 0
        {
            return Err("require lease > timeout > 0, attempts/backoff/capacity > 0, and max backoff >= initial backoff".into());
        }
        for millis in [self.lease_ms, self.max_backoff_ms] {
            if i64::try_from(millis)
                .ok()
                .and_then(TimeDelta::try_milliseconds)
                .is_none()
            {
                return Err("duration exceeds supported timestamp range".into());
            }
        }
        Ok(())
    }

    fn backoff_ms(&self, attempt: u32) -> u64 {
        self.initial_backoff_ms
            .saturating_mul(
                1_u64
                    .checked_shl(attempt.saturating_sub(1))
                    .unwrap_or(u64::MAX),
            )
            .min(self.max_backoff_ms)
    }
}

/// Durable delivery counters; redelivery is counted as another attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOutboxCounters {
    pub attempts: u64,
    pub delivered: u64,
    pub retries_scheduled: u64,
    pub dead_lettered: u64,
    pub replayed: u64,
    pub leases_recovered: u64,
}

/// Backlog gauges and durable counters for one processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamOutboxMetrics {
    pub counters: StreamOutboxCounters,
    pub pending: usize,
    pub leased: usize,
    pub ready: usize,
    pub dead_letters: usize,
    pub oldest_pending_age_ms: u64,
}

/// A failed output retained durably for inspection, replay, or removal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDeadLetter<O> {
    pub entry: StreamOutboxEntry<O>,
    pub attempts: u32,
    pub failed_at: DateTime<Utc>,
    /// Bounded diagnostic; adapters should redact secrets from error messages.
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryLease {
    token: String,
    worker_id: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryState {
    attempts: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    lease: Option<DeliveryLease>,
    last_error: Option<String>,
    terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedOutboxState<O> {
    config: StreamOutboxConfig,
    deliveries: BTreeMap<String, DeliveryState>,
    dead_letters: Vec<StreamDeadLetter<O>>,
    counters: StreamOutboxCounters,
}

impl<O> ManagedOutboxState<O> {
    pub(crate) fn contains_dead_letter(&self, key: &str) -> bool {
        self.dead_letters
            .iter()
            .any(|letter| letter.entry.idempotency_key == key)
    }

    pub(crate) fn validate(&self, pending: &BTreeSet<&String>) -> Result<(), String> {
        self.config.validate()?;
        if self.dead_letters.len() > self.config.max_dead_letters {
            return Err("dead-letter capacity exceeded".into());
        }
        let mut keys = BTreeSet::new();
        for letter in &self.dead_letters {
            if letter.entry.idempotency_key.trim().is_empty()
                || !keys.insert(&letter.entry.idempotency_key)
                || pending.contains(&letter.entry.idempotency_key)
                || letter.attempts == 0
                || letter.attempts > self.config.max_attempts
                || letter.last_error.len() > 4096
            {
                return Err("invalid or duplicate dead letter".into());
            }
        }
        for (key, state) in &self.deliveries {
            if !pending.contains(key) || state.attempts > self.config.max_attempts {
                return Err("orphaned delivery state or invalid attempt count".into());
            }
            if state.attempts == 0
                || (state.terminal
                    && (state.lease.is_some()
                        || state.next_attempt_at.is_some()
                        || state.last_error.is_none()))
            {
                return Err("invalid terminal delivery state or zero attempt count".into());
            }
            if state
                .last_error
                .as_ref()
                .is_some_and(|error| error.len() > 4096)
            {
                return Err("delivery diagnostic exceeds limit".into());
            }
            if let Some(lease) = &state.lease
                && (lease.token.trim().is_empty()
                    || lease.worker_id.trim().is_empty()
                    || state.attempts == 0
                    || state.next_attempt_at.is_some())
            {
                return Err("invalid delivery lease".into());
            }
        }
        Ok(())
    }
}

/// Receiver failure classification. Permanent failures bypass retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDeliveryError {
    Retryable(String),
    Permanent(String),
}

/// Adapter for any downstream receiver, including an Acteon dispatch client.
#[async_trait]
pub trait StreamOutboxDelivery<O>: Send + Sync {
    /// Return success only after the receiver accepts the output durably.
    /// Preserve `entry.idempotency_key` when forwarding to the receiver.
    async fn deliver(&self, entry: &StreamOutboxEntry<O>) -> Result<(), StreamDeliveryError>;
}

/// Publish checkpoint outputs through any Acteon bus backend. The stable
/// idempotency key is forwarded in the `idempotency-key` header. Kafka itself
/// does not deduplicate this header; consuming receivers must honor it.
pub struct BusOutboxDelivery {
    backend: SharedBackend,
}

impl BusOutboxDelivery {
    #[must_use]
    pub fn new(backend: SharedBackend) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl StreamOutboxDelivery<BusMessage> for BusOutboxDelivery {
    async fn deliver(
        &self,
        entry: &StreamOutboxEntry<BusMessage>,
    ) -> Result<(), StreamDeliveryError> {
        let mut message = entry.payload.clone();
        message
            .headers
            .insert("idempotency-key".into(), entry.idempotency_key.clone());
        self.backend
            .produce(message)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                BusError::InvalidTopic(_) | BusError::Serialization(_) => {
                    StreamDeliveryError::Permanent(error.to_string())
                }
                _ => StreamDeliveryError::Retryable(error.to_string()),
            })
    }
}

#[derive(Debug, Error)]
pub enum StreamOutboxError {
    #[error(transparent)]
    Checkpoint(#[from] StreamCheckpointError),
    #[error("invalid outbox configuration: {0}")]
    InvalidConfig(String),
    #[error("worker ID cannot be empty")]
    EmptyWorkerId,
    #[error("outbox delivery policy differs from the persisted policy")]
    ConfigMismatch,
    #[error("managed outbox state was removed or changed outside the dispatcher")]
    MissingManagedState,
    #[error("outbox remained contended after bounded CAS retries")]
    Contended,
    #[error("dead-letter capacity reached; output remains pending")]
    DeadLetterCapacity,
    #[error("delivery lease expired or was replaced; result was not acknowledged")]
    LeaseLost,
    #[error("timestamp overflow")]
    TimestampOverflow,
    #[error("dead letter '{0}' does not exist")]
    UnknownDeadLetter(String),
}

/// Outcome of one dispatcher iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutboxDispatchResult {
    Idle,
    Delivered {
        idempotency_key: String,
    },
    RetryScheduled {
        idempotency_key: String,
        next_attempt_at: DateTime<Utc>,
    },
    DeadLettered {
        idempotency_key: String,
    },
}

struct Claim<O> {
    entry: StreamOutboxEntry<O>,
    token: String,
    expires_at: DateTime<Utc>,
}

enum Claimed<O> {
    Idle,
    Deliver(Claim<O>),
    DeadLettered(String),
}

/// Managed dispatcher over one processor checkpoint. Multiple instances may
/// share the key; CAS claims prevent concurrent ownership of a live lease.
/// Processor writers must reload/recompute on checkpoint conflicts.
pub struct StreamOutboxDispatcher<S, O> {
    coordinator: StreamCheckpointCoordinator<S, O>,
    worker_id: String,
    config: StreamOutboxConfig,
}

impl<S, O> StreamOutboxDispatcher<S, O>
where
    S: Clone + Serialize + DeserializeOwned + Send + Sync,
    O: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    /// Attach a worker and atomically persist its delivery policy. Version-one
    /// checkpoints are upgraded by the next write without losing pending data.
    pub async fn initialize(
        coordinator: StreamCheckpointCoordinator<S, O>,
        worker_id: impl Into<String>,
        config: StreamOutboxConfig,
    ) -> Result<Self, StreamOutboxError> {
        config
            .validate()
            .map_err(StreamOutboxError::InvalidConfig)?;
        let worker_id = worker_id.into();
        if worker_id.trim().is_empty() {
            return Err(StreamOutboxError::EmptyWorkerId);
        }
        let mut dispatcher = Self {
            coordinator,
            worker_id,
            config,
        };
        for _ in 0..CAS_RETRIES {
            dispatcher.coordinator.reload().await?;
            if let Some(managed) = &dispatcher.coordinator.snapshot.managed {
                if managed.config != dispatcher.config {
                    return Err(StreamOutboxError::ConfigMismatch);
                }
                return Ok(dispatcher);
            }
            let mut next = dispatcher.coordinator.snapshot.clone();
            next.managed = Some(ManagedOutboxState {
                config: dispatcher.config.clone(),
                deliveries: BTreeMap::new(),
                dead_letters: Vec::new(),
                counters: StreamOutboxCounters::default(),
            });
            bump(&mut next.generation)?;
            match dispatcher.coordinator.persist(next).await {
                Ok(()) => return Ok(dispatcher),
                Err(StreamCheckpointError::Conflict { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(StreamOutboxError::Contended)
    }

    /// Deliver at most one entry. Durable completion follows receiver success.
    /// A timeout or dropped future may have produced a downstream side effect;
    /// retry always retains the same idempotency key.
    pub async fn dispatch_once<D: StreamOutboxDelivery<O> + ?Sized>(
        &mut self,
        delivery: &D,
    ) -> Result<StreamOutboxDispatchResult, StreamOutboxError> {
        match self.claim(Utc::now()).await? {
            Claimed::Idle => Ok(StreamOutboxDispatchResult::Idle),
            Claimed::DeadLettered(idempotency_key) => {
                Ok(StreamOutboxDispatchResult::DeadLettered { idempotency_key })
            }
            Claimed::Deliver(claim) => {
                if claim.expires_at <= Utc::now() {
                    return Err(StreamOutboxError::LeaseLost);
                }
                let result = tokio::time::timeout(
                    Duration::from_millis(self.config.delivery_timeout_ms),
                    delivery.deliver(&claim.entry),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(StreamDeliveryError::Retryable("delivery timed out".into()))
                });
                self.complete(&claim, result, Utc::now()).await
            }
        }
    }

    /// Poll until cancellation. Store/admission errors return to the supervisor.
    /// Cancellation interrupts in-flight delivery; its durable lease is later
    /// recovered. Use distinct worker IDs and synchronized clocks on hosts.
    pub async fn run<D: StreamOutboxDelivery<O> + ?Sized>(
        &mut self,
        delivery: &D,
        poll_interval: Duration,
        cancellation: &CancellationToken,
    ) -> Result<(), StreamOutboxError> {
        if poll_interval.is_zero() {
            return Err(StreamOutboxError::InvalidConfig(
                "poll interval must be positive".into(),
            ));
        }
        loop {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Ok(()),
                result = self.dispatch_once(delivery) => {
                    if result? == StreamOutboxDispatchResult::Idle {
                        tokio::select! {
                            () = cancellation.cancelled() => return Ok(()),
                            () = tokio::time::sleep(poll_interval) => {},
                        }
                    }
                }
            }
        }
    }

    /// Refresh gauges and counters from durable state.
    pub async fn metrics(&mut self) -> Result<StreamOutboxMetrics, StreamOutboxError> {
        self.reload().await?;
        Ok(self.metrics_at(Utc::now()))
    }

    /// Last loaded dead letters; call `metrics` to refresh without mutation.
    #[must_use]
    pub fn dead_letters(&self) -> &[StreamDeadLetter<O>] {
        self.coordinator
            .snapshot
            .managed
            .as_ref()
            .map_or(&[], |managed| managed.dead_letters.as_slice())
    }

    /// Put a dead letter back into the pending outbox with the same key and a
    /// fresh attempt budget. Its original creation timestamp is preserved.
    pub async fn replay_dead_letter(&mut self, key: &str) -> Result<(), StreamOutboxError> {
        self.change_dead_letter(key, true).await
    }

    /// Explicitly discard a retained dead letter after operator inspection.
    pub async fn discard_dead_letter(&mut self, key: &str) -> Result<(), StreamOutboxError> {
        self.change_dead_letter(key, false).await
    }

    async fn reload(&mut self) -> Result<(), StreamOutboxError> {
        self.coordinator.reload().await?;
        let managed = self
            .coordinator
            .snapshot
            .managed
            .as_ref()
            .ok_or(StreamOutboxError::MissingManagedState)?;
        if managed.config != self.config {
            return Err(StreamOutboxError::ConfigMismatch);
        }
        Ok(())
    }

    fn managed(&self) -> &ManagedOutboxState<O> {
        self.coordinator
            .snapshot
            .managed
            .as_ref()
            .expect("dispatcher initialized managed state")
    }

    async fn claim(&mut self, now: DateTime<Utc>) -> Result<Claimed<O>, StreamOutboxError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let managed = next.managed.as_mut().expect("managed state persisted");
            let selected = next
                .outbox
                .iter()
                .find(|entry| {
                    managed
                        .deliveries
                        .get(&entry.idempotency_key)
                        .is_none_or(|state| {
                            state
                                .lease
                                .as_ref()
                                .is_none_or(|lease| lease.expires_at <= now)
                                && state.next_attempt_at.is_none_or(|ready| ready <= now)
                        })
                })
                .cloned();
            let Some(entry) = selected else {
                return Ok(Claimed::Idle);
            };
            let state = managed
                .deliveries
                .entry(entry.idempotency_key.clone())
                .or_default();
            if state.lease.is_some() {
                managed.counters.leases_recovered =
                    managed.counters.leases_recovered.saturating_add(1);
            }
            let outcome = if state.terminal || state.attempts >= self.config.max_attempts {
                let error = if state.lease.is_some() {
                    "delivery lease expired on final attempt".into()
                } else {
                    state
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "attempt budget exhausted".into())
                };
                dead_letter(managed, &entry, now, error)?;
                next.outbox
                    .retain(|output| output.idempotency_key != entry.idempotency_key);
                Claimed::DeadLettered(entry.idempotency_key)
            } else {
                let token = Uuid::new_v4().to_string();
                state.attempts += 1;
                state.next_attempt_at = None;
                let expires_at = add_ms(now, self.config.lease_ms)?;
                state.lease = Some(DeliveryLease {
                    token: token.clone(),
                    worker_id: self.worker_id.clone(),
                    expires_at,
                });
                managed.counters.attempts = managed.counters.attempts.saturating_add(1);
                Claimed::Deliver(Claim {
                    entry,
                    token,
                    expires_at,
                })
            };
            bump(&mut next.generation)?;
            match self.coordinator.persist(next).await {
                Ok(()) => return Ok(outcome),
                Err(StreamCheckpointError::Conflict { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(StreamOutboxError::Contended)
    }

    async fn complete(
        &mut self,
        claim: &Claim<O>,
        result: Result<(), StreamDeliveryError>,
        now: DateTime<Utc>,
    ) -> Result<StreamOutboxDispatchResult, StreamOutboxError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let managed = next.managed.as_mut().expect("managed state persisted");
            let key = &claim.entry.idempotency_key;
            let state = managed
                .deliveries
                .get_mut(key)
                .ok_or(StreamOutboxError::LeaseLost)?;
            if !state
                .lease
                .as_ref()
                .is_some_and(|lease| lease.token == claim.token && lease.expires_at > now)
            {
                return Err(StreamOutboxError::LeaseLost);
            }
            let mut capacity_blocked = false;
            let outcome = match &result {
                Ok(()) => {
                    managed.deliveries.remove(key);
                    managed.counters.delivered = managed.counters.delivered.saturating_add(1);
                    next.outbox.retain(|entry| &entry.idempotency_key != key);
                    StreamOutboxDispatchResult::Delivered {
                        idempotency_key: key.clone(),
                    }
                }
                Err(error) => {
                    let (permanent, message) = match error {
                        StreamDeliveryError::Permanent(message) => (true, message),
                        StreamDeliveryError::Retryable(message) => (false, message),
                    };
                    let message: String = message.chars().take(1024).collect();
                    if permanent || state.attempts >= self.config.max_attempts {
                        if managed.dead_letters.len() >= self.config.max_dead_letters {
                            state.lease = None;
                            state.last_error = Some(message);
                            state.terminal = true;
                            capacity_blocked = true;
                        } else {
                            dead_letter(managed, &claim.entry, now, message)?;
                            next.outbox.retain(|entry| &entry.idempotency_key != key);
                        }
                        StreamOutboxDispatchResult::DeadLettered {
                            idempotency_key: key.clone(),
                        }
                    } else {
                        let ready = add_ms(now, self.config.backoff_ms(state.attempts))?;
                        state.lease = None;
                        state.last_error = Some(message);
                        state.next_attempt_at = Some(ready);
                        managed.counters.retries_scheduled =
                            managed.counters.retries_scheduled.saturating_add(1);
                        StreamOutboxDispatchResult::RetryScheduled {
                            idempotency_key: key.clone(),
                            next_attempt_at: ready,
                        }
                    }
                }
            };
            bump(&mut next.generation)?;
            match self.coordinator.persist(next).await {
                Ok(()) => {
                    return if capacity_blocked {
                        Err(StreamOutboxError::DeadLetterCapacity)
                    } else {
                        Ok(outcome)
                    };
                }
                Err(StreamCheckpointError::Conflict { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(StreamOutboxError::Contended)
    }

    async fn change_dead_letter(
        &mut self,
        key: &str,
        replay: bool,
    ) -> Result<(), StreamOutboxError> {
        for _ in 0..CAS_RETRIES {
            self.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let limit = next.config().max_pending_outputs;
            let managed = next.managed.as_mut().expect("managed state persisted");
            let index = managed
                .dead_letters
                .iter()
                .position(|letter| letter.entry.idempotency_key == key)
                .ok_or_else(|| StreamOutboxError::UnknownDeadLetter(key.into()))?;
            if replay && next.outbox.len() >= limit {
                return Err(StreamCheckpointError::OutboxCapacity { limit }.into());
            }
            let letter = managed.dead_letters.remove(index);
            if replay {
                next.outbox.push(letter.entry);
                next.outbox
                    .sort_by(|a, b| a.idempotency_key.cmp(&b.idempotency_key));
                managed.counters.replayed = managed.counters.replayed.saturating_add(1);
            }
            bump(&mut next.generation)?;
            match self.coordinator.persist(next).await {
                Ok(()) => return Ok(()),
                Err(StreamCheckpointError::Conflict { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(StreamOutboxError::Contended)
    }

    fn metrics_at(&self, now: DateTime<Utc>) -> StreamOutboxMetrics {
        let managed = self.managed();
        let pending = self.coordinator.snapshot.pending_outputs();
        let leased = managed
            .deliveries
            .values()
            .filter(|state| {
                state
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at > now)
            })
            .count();
        let ready = pending
            .iter()
            .filter(|entry| {
                managed
                    .deliveries
                    .get(&entry.idempotency_key)
                    .is_none_or(|state| {
                        state
                            .lease
                            .as_ref()
                            .is_none_or(|lease| lease.expires_at <= now)
                            && state.next_attempt_at.is_none_or(|at| at <= now)
                    })
            })
            .count();
        let age = pending
            .iter()
            .map(|entry| {
                now.signed_duration_since(entry.created_at)
                    .num_milliseconds()
            })
            .max()
            .unwrap_or(0);
        StreamOutboxMetrics {
            counters: managed.counters.clone(),
            pending: pending.len(),
            leased,
            ready,
            dead_letters: managed.dead_letters.len(),
            oldest_pending_age_ms: u64::try_from(age).unwrap_or(0),
        }
    }
}

fn bump(generation: &mut u64) -> Result<(), StreamCheckpointError> {
    *generation = generation
        .checked_add(1)
        .ok_or(StreamCheckpointError::GenerationOverflow)?;
    Ok(())
}

fn add_ms(now: DateTime<Utc>, milliseconds: u64) -> Result<DateTime<Utc>, StreamOutboxError> {
    i64::try_from(milliseconds)
        .ok()
        .and_then(TimeDelta::try_milliseconds)
        .and_then(|duration| now.checked_add_signed(duration))
        .ok_or(StreamOutboxError::TimestampOverflow)
}

fn dead_letter<O: Clone>(
    managed: &mut ManagedOutboxState<O>,
    entry: &StreamOutboxEntry<O>,
    now: DateTime<Utc>,
    message: String,
) -> Result<(), StreamOutboxError> {
    if managed.dead_letters.len() >= managed.config.max_dead_letters {
        return Err(StreamOutboxError::DeadLetterCapacity);
    }
    let state = managed
        .deliveries
        .remove(&entry.idempotency_key)
        .expect("claimed output has delivery state");
    managed.dead_letters.push(StreamDeadLetter {
        entry: entry.clone(),
        attempts: state.attempts,
        failed_at: now,
        last_error: message,
    });
    managed.counters.dead_lettered = managed.counters.dead_lettered.saturating_add(1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use acteon_state::{
        StateStore,
        testing::faults::{FaultStore, FaultTiming, WriteOperation},
    };
    use acteon_state_memory::MemoryStateStore;
    use serde_json::{Value, json};

    use super::*;
    use crate::{StreamCheckpointConfig, stream_checkpoint_key};

    type Dispatcher = StreamOutboxDispatcher<Value, Value>;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn seed(store: Arc<dyn StateStore>, keys: &[&str]) {
        let mut coordinator = StreamCheckpointCoordinator::initialize(
            store,
            stream_checkpoint_key("test", "tenant", "processor"),
            json!({"count": 1}),
            StreamCheckpointConfig::default(),
        )
        .await
        .unwrap();
        coordinator
            .checkpoint(
                json!({"count": 2}),
                [],
                keys.iter().map(|key| StreamOutboxEntry {
                    idempotency_key: (*key).into(),
                    created_at: now(),
                    payload: json!({"key": key}),
                }),
            )
            .await
            .unwrap();
    }

    async fn worker(
        store: Arc<dyn StateStore>,
        id: &str,
        config: StreamOutboxConfig,
    ) -> Dispatcher {
        let coordinator = StreamCheckpointCoordinator::initialize(
            store,
            stream_checkpoint_key("test", "tenant", "processor"),
            Value::Null,
            StreamCheckpointConfig::default(),
        )
        .await
        .unwrap();
        Dispatcher::initialize(coordinator, id, config)
            .await
            .unwrap()
    }

    fn claim(result: Claimed<Value>) -> Claim<Value> {
        match result {
            Claimed::Deliver(claim) => claim,
            _ => panic!("expected delivery claim"),
        }
    }

    #[tokio::test]
    async fn competing_workers_have_one_live_owner_and_fence_expired_results() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut first = worker(store.clone(), "first", StreamOutboxConfig::default()).await;
        let mut second = worker(store.clone(), "second", StreamOutboxConfig::default()).await;
        let (a, b) = tokio::join!(first.claim(now()), second.claim(now()));
        let (old, owner, other) = match (a.unwrap(), b.unwrap()) {
            (Claimed::Deliver(c), Claimed::Idle) => (c, &mut first, &mut second),
            (Claimed::Idle, Claimed::Deliver(c)) => (c, &mut second, &mut first),
            _ => panic!("exactly one live owner expected"),
        };
        let later = add_ms(now(), 30_001).unwrap();
        let replacement = claim(other.claim(later).await.unwrap());
        assert_ne!(old.token, replacement.token);
        assert!(matches!(
            owner.complete(&old, Ok(()), later).await,
            Err(StreamOutboxError::LeaseLost)
        ));
        assert_eq!(other.coordinator.snapshot.pending_outputs().len(), 1);
        other.complete(&replacement, Ok(()), later).await.unwrap();
        let metrics = other.metrics_at(later);
        assert_eq!(metrics.pending, 0);
        assert_eq!(metrics.counters.attempts, 2);
        assert_eq!(metrics.counters.delivered, 1);
        assert_eq!(metrics.counters.leases_recovered, 1);
    }

    #[tokio::test]
    async fn retry_schedule_survives_restart_and_does_not_block_other_entries() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["a", "b"]).await;
        let mut first = worker(store.clone(), "first", StreamOutboxConfig::default()).await;
        let a = claim(first.claim(now()).await.unwrap());
        assert_eq!(a.entry.idempotency_key, "a");
        let result = first
            .complete(
                &a,
                Err(StreamDeliveryError::Retryable("unavailable".into())),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            StreamOutboxDispatchResult::RetryScheduled {
                idempotency_key: "a".into(),
                next_attempt_at: add_ms(now(), 1_000).unwrap(),
            }
        );
        drop(first);
        let mut recovered = worker(store, "replacement", StreamOutboxConfig::default()).await;
        let b = claim(recovered.claim(now()).await.unwrap());
        assert_eq!(b.entry.idempotency_key, "b");
        recovered.complete(&b, Ok(()), now()).await.unwrap();
        assert!(matches!(
            recovered.claim(add_ms(now(), 999).unwrap()).await.unwrap(),
            Claimed::Idle
        ));
        let retry = claim(
            recovered
                .claim(add_ms(now(), 1_000).unwrap())
                .await
                .unwrap(),
        );
        assert_eq!(retry.entry, a.entry);
        assert_eq!(recovered.managed().deliveries["a"].attempts, 2);
        assert_eq!(recovered.managed().counters.retries_scheduled, 1);
        assert_eq!(recovered.config.backoff_ms(2), 2_000);
        assert_eq!(recovered.config.backoff_ms(u32::MAX), 60_000);
    }

    #[tokio::test]
    async fn permanent_failure_is_retained_and_replay_preserves_identity() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store.clone(), "worker", StreamOutboxConfig::default()).await;
        let output = claim(dispatcher.claim(now()).await.unwrap());
        dispatcher
            .complete(
                &output,
                Err(StreamDeliveryError::Permanent("invalid".into())),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(dispatcher.coordinator.snapshot.pending_outputs(), []);
        drop(dispatcher);
        let mut dispatcher = worker(store, "restart", StreamOutboxConfig::default()).await;
        assert_eq!(dispatcher.dead_letters()[0].entry, output.entry);
        assert_eq!(dispatcher.dead_letters()[0].attempts, 1);
        dispatcher.replay_dead_letter("one").await.unwrap();
        assert_eq!(dispatcher.dead_letters(), []);
        let replay = claim(dispatcher.claim(now()).await.unwrap());
        assert_eq!(replay.entry, output.entry);
        assert_eq!(dispatcher.managed().deliveries["one"].attempts, 1);
        assert_eq!(dispatcher.managed().counters.replayed, 1);
    }

    #[tokio::test]
    async fn final_crashed_attempt_goes_to_dead_letter_without_another_invocation() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let config = StreamOutboxConfig {
            max_attempts: 1,
            ..Default::default()
        };
        let mut dispatcher = worker(store.clone(), "worker", config.clone()).await;
        let _abandoned = claim(dispatcher.claim(now()).await.unwrap());
        drop(dispatcher);
        let mut recovered = worker(store, "restart", config).await;
        let later = add_ms(now(), 30_000).unwrap();
        assert!(matches!(
            recovered.claim(later).await.unwrap(),
            Claimed::DeadLettered(_)
        ));
        assert_eq!(recovered.dead_letters()[0].attempts, 1);
        assert_eq!(recovered.managed().counters.attempts, 1);
    }

    #[tokio::test]
    async fn full_dead_letter_storage_retains_terminal_failure_without_redelivery() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["a", "b"]).await;
        let config = StreamOutboxConfig {
            max_dead_letters: 1,
            ..Default::default()
        };
        let mut dispatcher = worker(store.clone(), "worker", config.clone()).await;
        let a = claim(dispatcher.claim(now()).await.unwrap());
        dispatcher
            .complete(
                &a,
                Err(StreamDeliveryError::Permanent("a bad".into())),
                now(),
            )
            .await
            .unwrap();
        let b = claim(dispatcher.claim(now()).await.unwrap());
        assert!(matches!(
            dispatcher
                .complete(
                    &b,
                    Err(StreamDeliveryError::Permanent("b bad".into())),
                    now()
                )
                .await,
            Err(StreamOutboxError::DeadLetterCapacity)
        ));
        drop(dispatcher);
        let mut recovered = worker(store, "restart", config).await;
        assert!(matches!(
            recovered.claim(now()).await,
            Err(StreamOutboxError::DeadLetterCapacity)
        ));
        recovered.discard_dead_letter("a").await.unwrap();
        assert!(
            matches!(recovered.claim(now()).await.unwrap(), Claimed::DeadLettered(key) if key == "b")
        );
        assert_eq!(recovered.dead_letters()[0].last_error, "b bad");
        assert_eq!(recovered.managed().counters.attempts, 2);
    }

    #[tokio::test]
    async fn processor_updates_survive_delivery_and_stale_processors_conflict() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store.clone(), "worker", StreamOutboxConfig::default()).await;
        let mut processor = StreamCheckpointCoordinator::<Value, Value>::initialize(
            store,
            stream_checkpoint_key("test", "tenant", "processor"),
            Value::Null,
            StreamCheckpointConfig::default(),
        )
        .await
        .unwrap();
        let output = claim(dispatcher.claim(now()).await.unwrap());
        assert!(matches!(
            processor.checkpoint(json!(3), [], []).await,
            Err(StreamCheckpointError::Conflict { .. })
        ));
        processor.reload().await.unwrap();
        processor.checkpoint(json!(3), [], []).await.unwrap();
        dispatcher.complete(&output, Ok(()), now()).await.unwrap();
        assert_eq!(dispatcher.coordinator.snapshot.state(), &json!(3));
        assert_eq!(dispatcher.coordinator.snapshot.pending_outputs().len(), 0);
    }

    #[tokio::test]
    async fn managed_entries_cannot_be_manually_acked_or_reinserted_over_dead_letters() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store, "worker", StreamOutboxConfig::default()).await;
        assert!(matches!(
            dispatcher.coordinator.acknowledge_outputs(["one"]).await,
            Err(StreamCheckpointError::ManagedOutput)
        ));
        let output = claim(dispatcher.claim(now()).await.unwrap());
        dispatcher
            .complete(
                &output,
                Err(StreamDeliveryError::Permanent("bad".into())),
                now(),
            )
            .await
            .unwrap();
        assert!(matches!(
            dispatcher
                .coordinator
                .checkpoint(Value::Null, [], [output.entry])
                .await,
            Err(StreamCheckpointError::DuplicateOutput(_))
        ));
    }

    #[tokio::test]
    async fn version_one_checkpoint_is_upgraded_and_invalid_managed_snapshots_rejected() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let key = stream_checkpoint_key("test", "tenant", "processor");
        let mut old: Value =
            serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
        old["schema_version"] = json!(1);
        store.set(&key, &old.to_string(), None).await.unwrap();
        let mut dispatcher = worker(store.clone(), "worker", StreamOutboxConfig::default()).await;
        let output = claim(dispatcher.claim(now()).await.unwrap());
        let mut encoded: Value =
            serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
        assert_eq!(encoded["schema_version"], json!(2));
        encoded["managed"]["deliveries"]["orphan"] =
            encoded["managed"]["deliveries"]["one"].clone();
        store.set(&key, &encoded.to_string(), None).await.unwrap();
        assert!(matches!(
            dispatcher.complete(&output, Ok(()), now()).await,
            Err(StreamOutboxError::Checkpoint(
                StreamCheckpointError::InvalidManagedOutbox(_)
            ))
        ));
    }

    struct Receiver {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl StreamOutboxDelivery<Value> for Receiver {
        async fn deliver(&self, _: &StreamOutboxEntry<Value>) -> Result<(), StreamDeliveryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_claim_never_invokes_receiver_and_lost_claim_ack_recovers() {
        for timing in [FaultTiming::Before, FaultTiming::After] {
            let fault = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
            seed(fault.clone(), &["one"]).await;
            let mut dispatcher =
                worker(fault.clone(), "worker", StreamOutboxConfig::default()).await;
            fault
                .fail_next(
                    stream_checkpoint_key("test", "tenant", "processor").kind,
                    WriteOperation::CompareAndSwap,
                    timing,
                )
                .unwrap();
            let receiver = Receiver {
                calls: AtomicUsize::new(0),
            };
            assert!(dispatcher.dispatch_once(&receiver).await.is_err());
            assert_eq!(receiver.calls.load(Ordering::SeqCst), 0);
            dispatcher.reload().await.unwrap();
            assert_eq!(dispatcher.metrics_at(Utc::now()).pending, 1);
            let later = add_ms(Utc::now(), 30_001).unwrap();
            let output = claim(dispatcher.claim(later).await.unwrap());
            dispatcher.complete(&output, Ok(()), later).await.unwrap();
        }
    }

    #[tokio::test]
    async fn receiver_success_with_failed_completion_keeps_recoverable_output() {
        for timing in [FaultTiming::Before, FaultTiming::After] {
            let fault = Arc::new(FaultStore::new(Arc::new(MemoryStateStore::new())));
            seed(fault.clone(), &["one"]).await;
            let mut dispatcher =
                worker(fault.clone(), "worker", StreamOutboxConfig::default()).await;
            fault
                .fail_after_matches(
                    stream_checkpoint_key("test", "tenant", "processor").kind,
                    WriteOperation::CompareAndSwap,
                    timing,
                    1,
                )
                .unwrap();
            let receiver = Receiver {
                calls: AtomicUsize::new(0),
            };
            assert!(dispatcher.dispatch_once(&receiver).await.is_err());
            assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);
            let metrics = dispatcher.metrics().await.unwrap();
            if timing == FaultTiming::Before {
                assert_eq!(metrics.pending, 1);
                let output = claim(
                    dispatcher
                        .claim(add_ms(Utc::now(), 30_001).unwrap())
                        .await
                        .unwrap(),
                );
                assert_eq!(output.entry.idempotency_key, "one");
            } else {
                assert_eq!(metrics.pending, 0);
                assert_eq!(metrics.counters.delivered, 1);
            }
        }
    }

    struct HangingReceiver {
        started: tokio::sync::Notify,
    }
    #[async_trait]
    impl StreamOutboxDelivery<Value> for HangingReceiver {
        async fn deliver(&self, _: &StreamOutboxEntry<Value>) -> Result<(), StreamDeliveryError> {
            self.started.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn delivery_timeout_schedules_retry_and_cancellation_leaves_a_recoverable_lease() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store, "worker", StreamOutboxConfig::default()).await;
        let receiver = HangingReceiver {
            started: tokio::sync::Notify::new(),
        };
        assert!(matches!(
            dispatcher.dispatch_once(&receiver).await.unwrap(),
            StreamOutboxDispatchResult::RetryScheduled { .. }
        ));
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store, "cancel-worker", StreamOutboxConfig::default()).await;
        let receiver = HangingReceiver {
            started: tokio::sync::Notify::new(),
        };
        let cancellation = CancellationToken::new();
        let cancel = async {
            receiver.started.notified().await;
            cancellation.cancel();
        };
        let (result, ()) = tokio::join!(
            dispatcher.run(&receiver, Duration::from_millis(100), &cancellation),
            cancel
        );
        result.unwrap();
        let metrics = dispatcher.metrics().await.unwrap();
        assert_eq!(metrics.pending, 1);
        assert_eq!(metrics.leased, 1);
        assert_eq!(metrics.counters.attempts, 1);
        let later = add_ms(Utc::now(), 30_001).unwrap();
        let recovered = claim(dispatcher.claim(later).await.unwrap());
        dispatcher
            .complete(&recovered, Ok(()), later)
            .await
            .unwrap();
        assert_eq!(dispatcher.metrics_at(later).pending, 0);
    }

    #[tokio::test]
    async fn policies_are_validated_and_workers_cannot_change_persisted_policy() {
        assert!(
            StreamOutboxConfig {
                lease_ms: 20_000,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            StreamOutboxConfig {
                lease_ms: u64::MAX,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let _first = worker(store.clone(), "first", StreamOutboxConfig::default()).await;
        let coordinator = StreamCheckpointCoordinator::<Value, Value>::initialize(
            store,
            stream_checkpoint_key("test", "tenant", "processor"),
            Value::Null,
            StreamCheckpointConfig::default(),
        )
        .await
        .unwrap();
        assert!(matches!(
            Dispatcher::initialize(
                coordinator,
                "other",
                StreamOutboxConfig {
                    max_attempts: 2,
                    ..Default::default()
                }
            )
            .await,
            Err(StreamOutboxError::ConfigMismatch)
        ));
    }

    #[tokio::test]
    async fn transient_failures_exhaust_the_budget_and_expiry_fences_even_without_takeover() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let config = StreamOutboxConfig {
            max_attempts: 2,
            ..Default::default()
        };
        let mut dispatcher = worker(store, "worker", config).await;
        let first = claim(dispatcher.claim(now()).await.unwrap());
        assert!(matches!(
            dispatcher
                .complete(&first, Ok(()), add_ms(now(), 30_000).unwrap())
                .await,
            Err(StreamOutboxError::LeaseLost)
        ));
        dispatcher
            .complete(
                &first,
                Err(StreamDeliveryError::Retryable("try later".into())),
                now(),
            )
            .await
            .unwrap();
        let later = add_ms(now(), 1_000).unwrap();
        let second = claim(dispatcher.claim(later).await.unwrap());
        assert!(matches!(
            dispatcher
                .complete(
                    &second,
                    Err(StreamDeliveryError::Retryable("still unavailable".into())),
                    later
                )
                .await
                .unwrap(),
            StreamOutboxDispatchResult::DeadLettered { .. }
        ));
        assert_eq!(dispatcher.dead_letters()[0].attempts, 2);
        assert_eq!(dispatcher.dead_letters()[0].last_error, "still unavailable");
        assert_eq!(dispatcher.managed().counters.attempts, 2);
        assert_eq!(dispatcher.managed().counters.dead_lettered, 1);
    }

    #[tokio::test]
    async fn concurrent_completions_preserve_both_outputs_and_counters() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["a", "b"]).await;
        let mut first = worker(store.clone(), "first", StreamOutboxConfig::default()).await;
        let mut second = worker(store, "second", StreamOutboxConfig::default()).await;
        let a = claim(first.claim(now()).await.unwrap());
        let b = claim(second.claim(now()).await.unwrap());
        let (left, right) = tokio::join!(
            first.complete(&a, Ok(()), now()),
            second.complete(&b, Ok(()), now())
        );
        left.unwrap();
        right.unwrap();
        first.reload().await.unwrap();
        let metrics = first.metrics_at(now());
        assert_eq!(metrics.pending, 0);
        assert_eq!(metrics.counters.delivered, 2);
        assert_eq!(metrics.counters.attempts, 2);
    }

    #[tokio::test]
    async fn replay_capacity_rejection_preserves_the_dead_letter() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["a"]).await;
        let mut dispatcher = worker(store, "worker", StreamOutboxConfig::default()).await;
        let output = claim(dispatcher.claim(now()).await.unwrap());
        dispatcher
            .complete(
                &output,
                Err(StreamDeliveryError::Permanent("bad".into())),
                now(),
            )
            .await
            .unwrap();
        // Reopen a checkpoint with a one-entry producer bound.
        let mut next = dispatcher.coordinator.snapshot.clone();
        // Config is private to the checkpoint module: use its serialized contract.
        let mut encoded = serde_json::to_value(&next).unwrap();
        encoded["config"]["max_pending_outputs"] = json!(1);
        next = serde_json::from_value(encoded).unwrap();
        bump(&mut next.generation).unwrap();
        dispatcher.coordinator.persist(next).await.unwrap();
        dispatcher
            .coordinator
            .checkpoint(
                json!({}),
                [],
                [StreamOutboxEntry {
                    idempotency_key: "b".into(),
                    created_at: now(),
                    payload: json!({}),
                }],
            )
            .await
            .unwrap();
        let generation = dispatcher.coordinator.snapshot.generation();
        assert!(matches!(
            dispatcher.replay_dead_letter("a").await,
            Err(StreamOutboxError::Checkpoint(
                StreamCheckpointError::OutboxCapacity { .. }
            ))
        ));
        assert_eq!(dispatcher.coordinator.snapshot.generation(), generation);
        assert_eq!(dispatcher.dead_letters().len(), 1);
        assert_eq!(dispatcher.managed().counters.replayed, 0);
    }

    #[tokio::test]
    async fn bus_adapter_forwards_the_outbox_key_over_a_conflicting_header() {
        use crate::{BusBackend, MemoryBackend, ScanFrom};
        use futures::StreamExt;
        let bus = MemoryBackend::new();
        let topic = acteon_core::Topic::new("test", "tenant", "decisions");
        bus.create_topic(&topic).await.unwrap();
        let adapter = BusOutboxDelivery::new(bus.clone());
        adapter
            .deliver(&StreamOutboxEntry {
                idempotency_key: "stable-key".into(),
                created_at: now(),
                payload: BusMessage::new(topic.kafka_topic_name(), json!({"decision": "alert"}))
                    .with_header("idempotency-key", "incorrect"),
            })
            .await
            .unwrap();
        let mut messages = bus
            .scan_topic(&topic.kafka_topic_name(), ScanFrom::Earliest)
            .await
            .unwrap();
        let received = messages.next().await.unwrap().unwrap();
        assert_eq!(received.headers["idempotency-key"], "stable-key");
        assert_eq!(received.payload, json!({"decision": "alert"}));
    }

    #[tokio::test]
    async fn corrupted_terminal_state_is_rejected_and_removed_managed_state_is_an_error() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        seed(store.clone(), &["one"]).await;
        let mut dispatcher = worker(store.clone(), "worker", StreamOutboxConfig::default()).await;
        let _output = claim(dispatcher.claim(now()).await.unwrap());
        let key = stream_checkpoint_key("test", "tenant", "processor");
        let mut encoded: Value =
            serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
        encoded["managed"]["deliveries"]["one"]["terminal"] = json!(true);
        store.set(&key, &encoded.to_string(), None).await.unwrap();
        assert!(matches!(
            dispatcher.metrics().await,
            Err(StreamOutboxError::Checkpoint(
                StreamCheckpointError::InvalidManagedOutbox(_)
            ))
        ));
        encoded.as_object_mut().unwrap().remove("managed");
        store.set(&key, &encoded.to_string(), None).await.unwrap();
        assert!(matches!(
            dispatcher.metrics().await,
            Err(StreamOutboxError::MissingManagedState)
        ));
        assert_eq!(dispatcher.dead_letters(), []);
    }
}
