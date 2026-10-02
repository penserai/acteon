//! Agentic message bus transport and stream-processing primitives.
//!
//! Wraps Kafka (via `rdkafka`) behind a small trait so the rest of
//! Acteon can produce to and subscribe from topics without touching
//! Kafka's SDK directly. A matching in-memory backend lives beside it
//! so unit tests don't need a running broker.
//!
//! The crate also provides publish-edge JSON Schema validation and a
//! transport-independent, durable event-time window state machine for
//! correlating records from multiple sources.

pub mod backend;
pub mod config;
pub mod error;
pub mod kafka;
pub mod memory;
pub mod message;
pub mod schema;
#[doc(hidden)]
pub mod testing;
pub mod windowing;

pub use backend::{BusBackend, ScanFrom, ScanWatermarks, SharedBackend, SubscribeStream};
pub use config::{BusConfig, KafkaBusConfig};
pub use error::BusError;
pub use kafka::KafkaBackend;
pub use memory::MemoryBackend;
pub use message::{BusMessage, DeliveryReceipt, OffsetPosition, StartOffset};
pub use schema::{SchemaValidator, SchemaValidatorError, ValidationIssue};
pub use windowing::{
    EventTimeWindow, EventTimeWindowAggregator, EventTimeWindowConfig, EventTimeWindowError,
    EventTimeWindowSnapshot, EventTimeWindowStats, WindowIngestDisposition, WindowIngestResult,
    WindowRecord, WindowSourcePosition, WindowWatermarkLane,
};

pub use acteon_core::PartitionLag;
