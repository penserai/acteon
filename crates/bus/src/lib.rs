//! Agentic message bus transport and stream-processing primitives.
//!
//! Wraps Kafka (via `rdkafka`) behind a small trait so the rest of
//! Acteon can produce to and subscribe from topics without touching
//! Kafka's SDK directly. A matching in-memory backend lives beside it
//! so unit tests don't need a running broker.
//!
//! The crate also provides publish-edge JSON Schema validation, a durable
//! event-time window state machine, and CAS-protected stream checkpoints with
//! an idempotent output outbox with managed delivery workers.

pub mod backend;
pub mod checkpoint;
pub mod config;
pub mod error;
pub mod kafka;
mod kafka_subscription;
pub mod memory;
pub mod message;
pub mod outbox;
pub mod schema;
pub mod stage;
pub mod stage_source;
pub mod subscription;
#[doc(hidden)]
pub mod testing;
pub mod windowing;

pub use backend::{BusBackend, ScanFrom, ScanWatermarks, SharedBackend, SubscribeStream};
pub use checkpoint::{
    AcknowledgedStreamCheckpoint, PersistedStreamCheckpoint, StreamCheckpointConfig,
    StreamCheckpointCoordinator, StreamCheckpointError, StreamCheckpointSnapshot,
    StreamOutboxEntry, StreamPosition, StreamPositionLane, SubscriptionCheckpointBatch,
    stream_checkpoint_key,
};
pub use config::{BusConfig, KafkaBusConfig};
pub use error::BusError;
pub use kafka::KafkaBackend;
pub use memory::MemoryBackend;
pub use message::{BusMessage, DeliveryReceipt, OffsetPosition, StartOffset};
pub use outbox::{
    BusOutboxDelivery, StreamDeadLetter, StreamDeliveryError, StreamOutboxConfig,
    StreamOutboxCounters, StreamOutboxDelivery, StreamOutboxDispatchResult, StreamOutboxDispatcher,
    StreamOutboxError, StreamOutboxMetrics,
};
pub use schema::{SchemaValidator, SchemaValidatorError, ValidationIssue};
pub use subscription::{
    AcknowledgedSubscription, SubscriptionAck, SubscriptionConfig, SubscriptionDelivery,
    SubscriptionError, SubscriptionOwnership, SubscriptionReceipt,
};
pub use windowing::{
    EventTimeWindow, EventTimeWindowAggregator, EventTimeWindowConfig, EventTimeWindowError,
    EventTimeWindowSnapshot, EventTimeWindowStats, WindowIngestDisposition, WindowIngestResult,
    WindowRecord, WindowSourcePosition, WindowWatermarkLane,
};

pub use acteon_core::PartitionLag;

pub use stage::{
    ManagedStreamStage, StreamReplayAudit, StreamReplayStatus, StreamStageCommand,
    StreamStageConfig, StreamStageControlAudit, StreamStageControlRequest, StreamStageCounters,
    StreamStageError, StreamStageInput, StreamStageMetrics, StreamStageOperator,
    StreamStageProcessError, StreamStageProcessor, StreamStageResult, StreamStageTransition,
};
pub use stage_source::{
    LiveStreamStageSource, StreamStageRecord, StreamStageSource, StreamStageSourceError,
    StreamStageSubscription,
};

pub mod ingestion;
pub use ingestion::{
    StreamInputContract, StreamInputFailure, StreamInputPolicy, StreamPoisonPolicy,
    StreamQuarantinedInput,
};
