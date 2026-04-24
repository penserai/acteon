use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use acteon_core::Topic;

use crate::error::BusError;
use crate::message::{BusMessage, DeliveryReceipt, StartOffset};

/// Stream yielded by [`BusBackend::subscribe`]. Items are individual
/// `BusMessage`s; a transport error ends the stream.
pub type SubscribeStream =
    Pin<Box<dyn Stream<Item = Result<BusMessage, BusError>> + Send + 'static>>;

/// Narrow transport-layer trait for the bus.
///
/// Kept object-safe so callers can hold `Arc<dyn BusBackend>` without
/// caring whether the underlying impl is Kafka or in-memory.
#[async_trait]
pub trait BusBackend: Send + Sync + 'static {
    /// Create the backing topic. Idempotent: returns `Ok(())` when the
    /// topic already exists with a matching partition count.
    async fn create_topic(&self, topic: &Topic) -> Result<(), BusError>;

    /// Delete the backing topic.
    async fn delete_topic(&self, kafka_name: &str) -> Result<(), BusError>;

    /// Produce a single message. The returned receipt carries the
    /// broker-assigned partition, offset, and timestamp.
    async fn produce(&self, message: BusMessage) -> Result<DeliveryReceipt, BusError>;

    /// Subscribe to a topic. `group_id` is used for consumer-group
    /// semantics where Kafka supports them (Phase 1 is fire-and-forget,
    /// so subscribers don't yet commit offsets).
    async fn subscribe(
        &self,
        kafka_topic: &str,
        group_id: &str,
        from: StartOffset,
    ) -> Result<SubscribeStream, BusError>;
}

/// Shared-ownership handle for consumers that want to stash a backend
/// in app state without specifying the concrete type.
pub type SharedBackend = Arc<dyn BusBackend>;
