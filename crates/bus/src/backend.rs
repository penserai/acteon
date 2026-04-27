use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use acteon_core::{PartitionLag, Topic};

use crate::error::BusError;
use crate::message::{BusMessage, DeliveryReceipt, OffsetPosition, StartOffset};

/// Where a [`BusBackend::scan_topic`] should start reading. A simple
/// `Earliest` / `Latest` covers the open-ended replay case; the
/// `FromOffsets` variant resumes from explicit per-partition offsets,
/// which is how the conversation-replay endpoint paginates without
/// holding state on the server.
#[derive(Debug, Clone)]
pub enum ScanFrom {
    /// Read from the earliest retained record on every partition.
    Earliest,
    /// Read from the broker's high-water mark on every partition.
    Latest,
    /// Read each partition starting at the explicit offset given;
    /// partitions absent from the map are skipped (the caller has
    /// already drained them).
    FromOffsets(BTreeMap<i32, i64>),
}

/// Snapshot of a scan's position, captured by `scan_topic_watermarks`
/// or after a partial replay so a client can resume where it left off.
#[derive(Debug, Clone, Default)]
pub struct ScanWatermarks {
    /// Per-partition high-water marks at the time of capture.
    pub high_water_marks: BTreeMap<i32, i64>,
}

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

    /// Commit a `(partition, offset)` pair for the given consumer group.
    /// Phase 2 manual-ack subscriptions call this after the caller
    /// finishes processing each record.
    async fn commit_offset(
        &self,
        kafka_topic: &str,
        group_id: &str,
        position: OffsetPosition,
    ) -> Result<(), BusError>;

    /// Report per-partition consumer-group lag
    /// (`high_water_mark − committed`). Partitions with no committed
    /// offset report `committed = -1`.
    async fn consumer_lag(
        &self,
        kafka_topic: &str,
        group_id: &str,
    ) -> Result<Vec<PartitionLag>, BusError>;

    /// One-shot replay of a topic without joining a consumer group.
    ///
    /// Conceptually like [`Self::subscribe`] but uses partition
    /// assignment (Kafka's `assign()` API) instead of dynamic group
    /// membership. Two consequences matter for the bus's replay
    /// endpoints:
    ///
    /// 1. **No group metadata is created.** Repeated replays do not
    ///    accumulate dead consumer groups in `__consumer_offsets`.
    /// 2. **No offset commits.** This is purely a read; offsets are
    ///    not persisted to Kafka.
    ///
    /// Use this for ephemeral, one-off scans (e.g. conversation thread
    /// replay). For durable consumer-group semantics call
    /// [`Self::subscribe`] instead.
    async fn scan_topic(
        &self,
        kafka_topic: &str,
        from: ScanFrom,
    ) -> Result<SubscribeStream, BusError>;

    /// Snapshot the high-water mark of every partition. Replay
    /// endpoints call this once at the start of a scan so they can
    /// determine end-of-stream by comparing tracked offsets against
    /// the captured marks. Without this snapshot, an `assign`-based
    /// scan never naturally terminates — Kafka keeps the stream open
    /// waiting for new records.
    async fn scan_topic_watermarks(&self, kafka_topic: &str) -> Result<ScanWatermarks, BusError>;
}

/// Shared-ownership handle for consumers that want to stash a backend
/// in app state without specifying the concrete type.
pub type SharedBackend = Arc<dyn BusBackend>;
