//! Agentic message bus transport (Phase 1).
//!
//! Wraps Kafka (via `rdkafka`) behind a small trait so the rest of
//! Acteon can produce to and subscribe from topics without touching
//! Kafka's SDK directly. A matching in-memory backend lives beside it
//! so unit tests don't need a running broker.
//!
//! Phase 1 intentionally keeps the surface narrow:
//!
//! * [`BusBackend::create_topic`] / [`BusBackend::delete_topic`]
//! * [`BusBackend::produce`]
//! * [`BusBackend::subscribe`] returning a stream of [`BusMessage`]
//!
//! Consumer groups, offsets, schema enforcement, and typed envelopes
//! are the job of later phases. The trait is deliberately shaped so
//! Phase 2 can add `ack`/`commit` methods without breaking callers.

pub mod backend;
pub mod config;
pub mod error;
pub mod kafka;
pub mod memory;
pub mod message;

pub use backend::{BusBackend, SharedBackend, SubscribeStream};
pub use config::{BusConfig, KafkaBusConfig};
pub use error::BusError;
pub use kafka::KafkaBackend;
pub use memory::MemoryBackend;
pub use message::{BusMessage, DeliveryReceipt, StartOffset};
