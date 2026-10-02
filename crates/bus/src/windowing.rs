//! Durable, transport-independent event-time windows for multi-source streams.
//!
//! The windowing state machine consumes normalized [`WindowRecord`] values, so
//! callers can extract event IDs, source names, correlation keys, and event
//! timestamps from any JSON envelope without coupling this module to one
//! observability or business schema.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::BusMessage;

const SNAPSHOT_VERSION: u16 = 1;
const DEFAULT_MAX_OPEN_WINDOWS: usize = 10_000;
const DEFAULT_MAX_RECORDS_PER_WINDOW: usize = 10_000;
const DEFAULT_MAX_RECORDS_PER_SOURCE_PER_WINDOW: usize = 10_000;
const DEFAULT_MAX_TRACKED_EVENT_IDS: usize = 100_000;

/// Broker position carried into a completed window for atomic checkpointing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSourcePosition {
    /// Full source topic name.
    pub topic: String,
    /// Broker partition.
    pub partition: i32,
    /// Offset within the partition.
    pub offset: i64,
}

/// One independently progressing input used to compute the global watermark.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowWatermarkLane {
    /// One logical clock for a nonpartitioned or externally coordinated source.
    Source { source: String },
    /// One Kafka-style topic partition clock.
    Partition {
        source: String,
        topic: String,
        partition: i32,
    },
}

impl WindowWatermarkLane {
    /// Construct a logical source clock.
    #[must_use]
    pub fn source(source: impl Into<String>) -> Self {
        Self::Source {
            source: source.into(),
        }
    }

    /// Construct a partition-specific source clock.
    #[must_use]
    pub fn partition(source: impl Into<String>, topic: impl Into<String>, partition: i32) -> Self {
        Self::Partition {
            source: source.into(),
            topic: topic.into(),
            partition,
        }
    }

    fn source_name(&self) -> &str {
        match self {
            Self::Source { source } | Self::Partition { source, .. } => source,
        }
    }
}

/// A normalized event admitted to the windowing state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRecord {
    /// Stable, globally unique event identifier used for replay deduplication.
    pub event_id: String,
    /// Configured input source, such as `metrics`, `traces`, or `orders`.
    pub source: String,
    /// Key used to join records from different sources.
    pub correlation_key: String,
    /// Application event time. Window assignment never uses arrival time.
    pub observed_at: DateTime<Utc>,
    /// Original JSON envelope or an application-specific normalized payload.
    pub payload: Value,
    /// Source position that can be committed after the snapshot is durable.
    pub position: WindowSourcePosition,
}

impl WindowRecord {
    /// Normalize a consumed bus message while retaining its broker position.
    pub fn from_bus_message(
        message: BusMessage,
        event_id: impl Into<String>,
        source: impl Into<String>,
        correlation_key: impl Into<String>,
        observed_at: DateTime<Utc>,
    ) -> Result<Self, EventTimeWindowError> {
        let partition = message
            .partition
            .ok_or_else(|| EventTimeWindowError::MissingPosition {
                topic: message.topic.clone(),
                field: "partition",
            })?;
        let offset = message
            .offset
            .ok_or_else(|| EventTimeWindowError::MissingPosition {
                topic: message.topic.clone(),
                field: "offset",
            })?;
        Ok(Self {
            event_id: event_id.into(),
            source: source.into(),
            correlation_key: correlation_key.into(),
            observed_at,
            payload: message.payload,
            position: WindowSourcePosition {
                topic: message.topic,
                partition,
                offset,
            },
        })
    }
}

/// Limits and event-time behavior for a multi-source window operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTimeWindowConfig {
    /// Every source that participates in completeness and the global watermark.
    pub sources: BTreeSet<String>,
    /// Independently progressing clocks used to calculate the watermark.
    pub watermark_lanes: BTreeSet<WindowWatermarkLane>,
    /// Width of each fixed, epoch-aligned event-time window.
    pub window_size: Duration,
    /// Time accepted behind the slowest source watermark.
    pub allowed_lateness: Duration,
    /// Emit as soon as at least one record from every source is present.
    pub emit_when_complete: bool,
    /// Hard limit on concurrently open correlation windows.
    pub max_open_windows: usize,
    /// Hard limit on records retained in one correlation window.
    pub max_records_per_window: usize,
    /// Hard limit on records retained from one source in one window.
    pub max_records_per_source_per_window: usize,
    /// Hard limit on event IDs retained for replay deduplication.
    pub max_tracked_event_ids: usize,
}

impl EventTimeWindowConfig {
    /// Create a bounded fixed-window configuration.
    pub fn new<I, S>(
        sources: I,
        window_size: Duration,
        allowed_lateness: Duration,
    ) -> Result<Self, EventTimeWindowError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let sources = sources.into_iter().map(Into::into).collect::<BTreeSet<_>>();
        let config = Self {
            watermark_lanes: sources
                .iter()
                .cloned()
                .map(WindowWatermarkLane::source)
                .collect(),
            sources,
            window_size,
            allowed_lateness,
            emit_when_complete: true,
            max_open_windows: DEFAULT_MAX_OPEN_WINDOWS,
            max_records_per_window: DEFAULT_MAX_RECORDS_PER_WINDOW,
            max_records_per_source_per_window: DEFAULT_MAX_RECORDS_PER_SOURCE_PER_WINDOW,
            max_tracked_event_ids: DEFAULT_MAX_TRACKED_EVENT_IDS,
        };
        config.validate()?;
        Ok(config)
    }

    /// Track every declared topic partition for one source independently.
    ///
    /// Call this after partition discovery and before constructing the
    /// aggregator. The source-level lane is replaced atomically.
    pub fn set_source_partitions<I, T>(
        &mut self,
        source: &str,
        partitions: I,
    ) -> Result<(), EventTimeWindowError>
    where
        I: IntoIterator<Item = (T, i32)>,
        T: Into<String>,
    {
        if !self.sources.contains(source) {
            return Err(EventTimeWindowError::UnknownSource(source.to_owned()));
        }
        let replacements = partitions
            .into_iter()
            .map(|(topic, partition)| WindowWatermarkLane::partition(source, topic, partition))
            .collect::<BTreeSet<_>>();
        if replacements.is_empty() {
            return Err(EventTimeWindowError::InvalidWatermarkLanes(
                "at least one partition is required".to_owned(),
            ));
        }
        let mut candidate = self.clone();
        candidate
            .watermark_lanes
            .retain(|lane| lane.source_name() != source);
        candidate.watermark_lanes.extend(replacements);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    fn validate(&self) -> Result<(), EventTimeWindowError> {
        if self.sources.is_empty() {
            return Err(EventTimeWindowError::NoSources);
        }
        if self.sources.iter().any(|source| source.trim().is_empty()) {
            return Err(EventTimeWindowError::EmptySource);
        }
        for source in &self.sources {
            let lanes = self
                .watermark_lanes
                .iter()
                .filter(|lane| lane.source_name() == source)
                .collect::<Vec<_>>();
            let logical = lanes
                .iter()
                .any(|lane| matches!(lane, WindowWatermarkLane::Source { .. }));
            if lanes.is_empty() || (logical && lanes.len() != 1) {
                return Err(EventTimeWindowError::InvalidWatermarkLanes(format!(
                    "source '{source}' must use one logical lane or one or more partition lanes"
                )));
            }
        }
        if self.watermark_lanes.iter().any(|lane| {
            !self.sources.contains(lane.source_name())
                || lane.source_name().trim().is_empty()
                || matches!(lane, WindowWatermarkLane::Partition { topic, .. } if topic.trim().is_empty())
        }) {
            return Err(EventTimeWindowError::InvalidWatermarkLanes(
                "a lane has an unknown source or empty topic".to_owned(),
            ));
        }
        if self.window_size <= Duration::zero() {
            return Err(EventTimeWindowError::InvalidDuration {
                field: "window_size",
            });
        }
        if self.allowed_lateness < Duration::zero() {
            return Err(EventTimeWindowError::InvalidDuration {
                field: "allowed_lateness",
            });
        }
        for (field, value) in [
            ("max_open_windows", self.max_open_windows),
            ("max_records_per_window", self.max_records_per_window),
            (
                "max_records_per_source_per_window",
                self.max_records_per_source_per_window,
            ),
            ("max_tracked_event_ids", self.max_tracked_event_ids),
        ] {
            if value == 0 {
                return Err(EventTimeWindowError::ZeroCapacity { field });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowKey {
    correlation_key: String,
    starts_at_ms: i64,
}

/// One correlated fixed window, grouped by source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventTimeWindow {
    /// Join key shared by every record in this window.
    pub correlation_key: String,
    /// Inclusive event-time boundary.
    pub starts_at: DateTime<Utc>,
    /// Exclusive event-time boundary.
    pub ends_at: DateTime<Utc>,
    /// Records retained per configured source. Emitted records are sorted
    /// deterministically by event time and broker position.
    pub records: BTreeMap<String, Vec<WindowRecord>>,
}

impl EventTimeWindow {
    /// Stable key for an idempotent outbox or downstream dispatch.
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.correlation_key.len(),
            self.correlation_key,
            self.starts_at.timestamp_millis()
        )
    }

    /// Records supplied by one source.
    #[must_use]
    pub fn records_for(&self, source: &str) -> &[WindowRecord] {
        self.records.get(source).map_or(&[], Vec::as_slice)
    }

    /// Configured sources that did not contribute a record.
    #[must_use]
    pub fn missing_sources(&self, sources: &BTreeSet<String>) -> Vec<String> {
        sources
            .iter()
            .filter(|source| self.records_for(source).is_empty())
            .cloned()
            .collect()
    }

    /// Whether every configured source contributed at least one record.
    #[must_use]
    pub fn is_complete(&self, sources: &BTreeSet<String>) -> bool {
        self.missing_sources(sources).is_empty()
    }

    fn record_count(&self) -> usize {
        self.records.values().map(Vec::len).sum()
    }
}

/// Outcome for the input record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowIngestDisposition {
    /// The record entered an open window.
    Accepted,
    /// The event ID was already present in recovery state.
    Duplicate,
    /// The corresponding window was already behind the watermark or emitted.
    Late,
}

/// Input disposition plus any windows made ready by the same transition.
#[derive(Debug)]
pub struct WindowIngestResult {
    /// Outcome for the input record.
    pub disposition: WindowIngestDisposition,
    /// Complete or watermark-closed windows, sorted deterministically.
    pub emitted: Vec<EventTimeWindow>,
}

/// Cumulative operator counters, included in recovery snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventTimeWindowStats {
    pub accepted_records: u64,
    pub duplicate_records: u64,
    pub late_records: u64,
    pub complete_windows: u64,
    pub incomplete_windows: u64,
}

/// Versioned, deterministic representation of the full recovery state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventTimeWindowSnapshot {
    schema_version: u16,
    sources: Vec<String>,
    watermark_lanes: Vec<WindowWatermarkLane>,
    window_size_ms: i64,
    allowed_lateness_ms: i64,
    emit_when_complete: bool,
    max_open_windows: usize,
    max_records_per_window: usize,
    max_records_per_source_per_window: usize,
    max_tracked_event_ids: usize,
    windows: Vec<(WindowKey, EventTimeWindow)>,
    finalized: Vec<(WindowKey, DateTime<Utc>)>,
    seen_event_ids: Vec<(String, DateTime<Utc>)>,
    source_high_water: Vec<(WindowWatermarkLane, DateTime<Utc>)>,
    stats: EventTimeWindowStats,
}

/// Errors that prevent a deterministic window transition.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EventTimeWindowError {
    #[error("at least one event source is required")]
    NoSources,
    #[error("event source names cannot be empty")]
    EmptySource,
    #[error("{field} must have a valid non-negative duration")]
    InvalidDuration { field: &'static str },
    #[error("{field} must be greater than zero")]
    ZeroCapacity { field: &'static str },
    #[error("message {topic} has no broker {field}")]
    MissingPosition { topic: String, field: &'static str },
    #[error("window record {field} cannot be empty")]
    EmptyRecordField { field: &'static str },
    #[error("source '{0}' is not configured for this window operator")]
    UnknownSource(String),
    #[error("invalid watermark lanes: {0}")]
    InvalidWatermarkLanes(String),
    #[error(
        "record does not match a configured watermark lane: {signal_source}/{topic}/{partition}"
    )]
    UnknownWatermarkLane {
        signal_source: String,
        topic: String,
        partition: i32,
    },
    #[error("event time is outside chrono's supported range")]
    EventTimeOutOfRange,
    #[error("open-window capacity {limit} exceeded")]
    OpenWindowCapacity { limit: usize },
    #[error("window record capacity {limit} exceeded for correlation key '{correlation_key}'")]
    WindowRecordCapacity {
        limit: usize,
        correlation_key: String,
    },
    #[error(
        "source record capacity {limit} exceeded for source '{signal_source}' and correlation key '{correlation_key}'"
    )]
    SourceRecordCapacity {
        limit: usize,
        signal_source: String,
        correlation_key: String,
    },
    #[error("tracked event-ID capacity {limit} exceeded")]
    EventIdCapacity { limit: usize },
    #[error("unsupported event-time window snapshot version {0}")]
    UnsupportedSnapshotVersion(u16),
    #[error("event-time window snapshot contains duplicate {collection} entry")]
    DuplicateSnapshotEntry { collection: &'static str },
    #[error("event-time window snapshot is inconsistent: {0}")]
    InvalidSnapshot(String),
}

/// Fixed event-time windows with multi-source watermarks and durable state.
pub struct EventTimeWindowAggregator {
    config: EventTimeWindowConfig,
    windows: BTreeMap<WindowKey, EventTimeWindow>,
    finalized: BTreeMap<WindowKey, DateTime<Utc>>,
    seen_event_ids: BTreeMap<String, DateTime<Utc>>,
    source_high_water: BTreeMap<WindowWatermarkLane, DateTime<Utc>>,
    stats: EventTimeWindowStats,
}

impl EventTimeWindowAggregator {
    /// Create an empty operator from validated configuration.
    pub fn new(config: EventTimeWindowConfig) -> Result<Self, EventTimeWindowError> {
        config.validate()?;
        Ok(Self {
            config,
            windows: BTreeMap::new(),
            finalized: BTreeMap::new(),
            seen_event_ids: BTreeMap::new(),
            source_high_water: BTreeMap::new(),
            stats: EventTimeWindowStats::default(),
        })
    }

    /// Immutable runtime configuration.
    #[must_use]
    pub fn config(&self) -> &EventTimeWindowConfig {
        &self.config
    }

    /// Current global event-time watermark.
    #[must_use]
    pub fn watermark(&self) -> Option<DateTime<Utc>> {
        watermark_for(
            &self.config.watermark_lanes,
            &self.source_high_water,
            self.config.allowed_lateness,
        )
    }

    /// Cumulative counters.
    #[must_use]
    pub fn stats(&self) -> &EventTimeWindowStats {
        &self.stats
    }

    /// Number of correlation windows currently retained in memory.
    #[must_use]
    pub fn open_window_count(&self) -> usize {
        self.windows.len()
    }

    /// Ingest one normalized record and apply a deterministic state transition.
    pub fn ingest(
        &mut self,
        record: WindowRecord,
    ) -> Result<WindowIngestResult, EventTimeWindowError> {
        self.validate_record(&record)?;
        if self.seen_event_ids.contains_key(&record.event_id) {
            self.stats.duplicate_records = self.stats.duplicate_records.saturating_add(1);
            return Ok(WindowIngestResult {
                disposition: WindowIngestDisposition::Duplicate,
                emitted: Vec::new(),
            });
        }

        let (key, starts_at, ends_at) =
            self.window_for(&record.correlation_key, record.observed_at)?;
        let watermark_lane = self.watermark_lane(&record)?;
        let mut prospective_high_water = self.source_high_water.clone();
        prospective_high_water
            .entry(watermark_lane)
            .and_modify(|current| *current = (*current).max(record.observed_at))
            .or_insert(record.observed_at);
        let prospective_watermark = watermark_for(
            &self.config.watermark_lanes,
            &prospective_high_water,
            self.config.allowed_lateness,
        );
        let is_late = self.finalized.contains_key(&key)
            || prospective_watermark.is_some_and(|watermark| ends_at <= watermark);
        self.validate_admission(&record, &key, is_late, prospective_watermark)?;

        self.source_high_water = prospective_high_water;
        self.seen_event_ids
            .insert(record.event_id.clone(), record.observed_at);
        let mut emitted = self.close_eligible_windows();
        if is_late {
            self.stats.late_records = self.stats.late_records.saturating_add(1);
            self.prune_replay_state();
            return Ok(WindowIngestResult {
                disposition: WindowIngestDisposition::Late,
                emitted,
            });
        }

        let source = record.source.clone();
        let window = self
            .windows
            .entry(key.clone())
            .or_insert_with(|| EventTimeWindow {
                correlation_key: key.correlation_key.clone(),
                starts_at,
                ends_at,
                records: BTreeMap::new(),
            });
        window.records.entry(source).or_default().push(record);
        self.stats.accepted_records = self.stats.accepted_records.saturating_add(1);
        if self.config.emit_when_complete && window.is_complete(&self.config.sources) {
            emitted.push(self.remove_window(&key, true));
        }
        self.prune_replay_state();
        Ok(WindowIngestResult {
            disposition: WindowIngestDisposition::Accepted,
            emitted,
        })
    }

    /// Advance an idle source explicitly and close windows behind the watermark.
    pub fn advance_source_watermark(
        &mut self,
        source: &str,
        observed_through: DateTime<Utc>,
    ) -> Result<Vec<EventTimeWindow>, EventTimeWindowError> {
        self.advance_watermark(&WindowWatermarkLane::source(source), observed_through)
    }

    /// Advance one configured logical or partition watermark lane.
    pub fn advance_watermark(
        &mut self,
        lane: &WindowWatermarkLane,
        observed_through: DateTime<Utc>,
    ) -> Result<Vec<EventTimeWindow>, EventTimeWindowError> {
        if !self.config.watermark_lanes.contains(lane) {
            return Err(EventTimeWindowError::InvalidWatermarkLanes(format!(
                "lane {lane:?} is not configured"
            )));
        }
        self.source_high_water
            .entry(lane.clone())
            .and_modify(|current| *current = (*current).max(observed_through))
            .or_insert(observed_through);
        let emitted = self.close_eligible_windows();
        self.prune_replay_state();
        Ok(emitted)
    }

    /// Drain every open window, for a finite stream or controlled shutdown.
    pub fn finish(&mut self) -> Vec<EventTimeWindow> {
        let keys = self.windows.keys().cloned().collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let complete = self.windows[&key].is_complete(&self.config.sources);
                self.remove_window(&key, complete)
            })
            .collect()
    }

    /// Capture all state required to resume without reopening emitted windows.
    #[must_use]
    pub fn snapshot(&self) -> EventTimeWindowSnapshot {
        EventTimeWindowSnapshot {
            schema_version: SNAPSHOT_VERSION,
            sources: self.config.sources.iter().cloned().collect(),
            watermark_lanes: self.config.watermark_lanes.iter().cloned().collect(),
            window_size_ms: self.config.window_size.num_milliseconds(),
            allowed_lateness_ms: self.config.allowed_lateness.num_milliseconds(),
            emit_when_complete: self.config.emit_when_complete,
            max_open_windows: self.config.max_open_windows,
            max_records_per_window: self.config.max_records_per_window,
            max_records_per_source_per_window: self.config.max_records_per_source_per_window,
            max_tracked_event_ids: self.config.max_tracked_event_ids,
            windows: self
                .windows
                .iter()
                .map(|(key, window)| (key.clone(), window.clone()))
                .collect(),
            finalized: self
                .finalized
                .iter()
                .map(|(key, ends_at)| (key.clone(), *ends_at))
                .collect(),
            seen_event_ids: self
                .seen_event_ids
                .iter()
                .map(|(event_id, observed_at)| (event_id.clone(), *observed_at))
                .collect(),
            source_high_water: self
                .source_high_water
                .iter()
                .map(|(lane, observed_at)| (lane.clone(), *observed_at))
                .collect(),
            stats: self.stats.clone(),
        }
    }

    /// Restore and validate a snapshot before accepting new records.
    pub fn restore(snapshot: EventTimeWindowSnapshot) -> Result<Self, EventTimeWindowError> {
        if snapshot.schema_version != SNAPSHOT_VERSION {
            return Err(EventTimeWindowError::UnsupportedSnapshotVersion(
                snapshot.schema_version,
            ));
        }
        let source_count = snapshot.sources.len();
        let sources = snapshot.sources.into_iter().collect::<BTreeSet<_>>();
        if sources.len() != source_count {
            return Err(EventTimeWindowError::DuplicateSnapshotEntry {
                collection: "source",
            });
        }
        let watermark_lane_count = snapshot.watermark_lanes.len();
        let watermark_lanes = snapshot
            .watermark_lanes
            .into_iter()
            .collect::<BTreeSet<_>>();
        if watermark_lanes.len() != watermark_lane_count {
            return Err(EventTimeWindowError::DuplicateSnapshotEntry {
                collection: "watermark lane",
            });
        }
        let config = EventTimeWindowConfig {
            sources,
            watermark_lanes,
            window_size: Duration::milliseconds(snapshot.window_size_ms),
            allowed_lateness: Duration::milliseconds(snapshot.allowed_lateness_ms),
            emit_when_complete: snapshot.emit_when_complete,
            max_open_windows: snapshot.max_open_windows,
            max_records_per_window: snapshot.max_records_per_window,
            max_records_per_source_per_window: snapshot.max_records_per_source_per_window,
            max_tracked_event_ids: snapshot.max_tracked_event_ids,
        };
        config.validate()?;
        let windows = collect_unique(snapshot.windows, "window")?;
        let finalized = collect_unique(snapshot.finalized, "finalized window")?;
        let seen_event_ids = collect_unique(snapshot.seen_event_ids, "event ID")?;
        let source_high_water = collect_unique(snapshot.source_high_water, "source watermark")?;
        let mut restored = Self {
            config,
            windows,
            finalized,
            seen_event_ids,
            source_high_water,
            stats: snapshot.stats,
        };
        restored.validate_restored_state()?;
        restored.prune_replay_state();
        Ok(restored)
    }

    fn validate_record(&self, record: &WindowRecord) -> Result<(), EventTimeWindowError> {
        for (field, value) in [
            ("event_id", record.event_id.as_str()),
            ("source", record.source.as_str()),
            ("correlation_key", record.correlation_key.as_str()),
            ("position.topic", record.position.topic.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(EventTimeWindowError::EmptyRecordField { field });
            }
        }
        if !self.config.sources.contains(&record.source) {
            return Err(EventTimeWindowError::UnknownSource(record.source.clone()));
        }
        Ok(())
    }

    fn validate_admission(
        &self,
        record: &WindowRecord,
        key: &WindowKey,
        is_late: bool,
        prospective_watermark: Option<DateTime<Utc>>,
    ) -> Result<(), EventTimeWindowError> {
        let closable = prospective_watermark.map_or(0, |watermark| {
            self.windows
                .values()
                .filter(|window| window.ends_at <= watermark)
                .count()
        });
        let creates_window = !is_late && !self.windows.contains_key(key);
        if creates_window
            && self.windows.len().saturating_sub(closable) >= self.config.max_open_windows
        {
            return Err(EventTimeWindowError::OpenWindowCapacity {
                limit: self.config.max_open_windows,
            });
        }
        if !is_late
            && self
                .windows
                .get(key)
                .is_some_and(|window| window.record_count() >= self.config.max_records_per_window)
        {
            return Err(EventTimeWindowError::WindowRecordCapacity {
                limit: self.config.max_records_per_window,
                correlation_key: record.correlation_key.clone(),
            });
        }
        if !is_late
            && self.windows.get(key).is_some_and(|window| {
                window.records_for(&record.source).len()
                    >= self.config.max_records_per_source_per_window
            })
        {
            return Err(EventTimeWindowError::SourceRecordCapacity {
                limit: self.config.max_records_per_source_per_window,
                signal_source: record.source.clone(),
                correlation_key: record.correlation_key.clone(),
            });
        }
        let prunable_ids = prospective_watermark.map_or(0, |watermark| {
            self.seen_event_ids
                .values()
                .filter(|event_time| {
                    self.window_end(**event_time)
                        .is_ok_and(|window_end| window_end <= watermark)
                })
                .count()
        });
        if self.seen_event_ids.len().saturating_sub(prunable_ids)
            >= self.config.max_tracked_event_ids
        {
            return Err(EventTimeWindowError::EventIdCapacity {
                limit: self.config.max_tracked_event_ids,
            });
        }
        Ok(())
    }

    fn watermark_lane(
        &self,
        record: &WindowRecord,
    ) -> Result<WindowWatermarkLane, EventTimeWindowError> {
        let partition = WindowWatermarkLane::partition(
            &record.source,
            &record.position.topic,
            record.position.partition,
        );
        if self.config.watermark_lanes.contains(&partition) {
            return Ok(partition);
        }
        let source = WindowWatermarkLane::source(&record.source);
        if self.config.watermark_lanes.contains(&source) {
            return Ok(source);
        }
        Err(EventTimeWindowError::UnknownWatermarkLane {
            signal_source: record.source.clone(),
            topic: record.position.topic.clone(),
            partition: record.position.partition,
        })
    }

    fn window_for(
        &self,
        correlation_key: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<(WindowKey, DateTime<Utc>, DateTime<Utc>), EventTimeWindowError> {
        let width_ms = self.config.window_size.num_milliseconds();
        let starts_at_ms = observed_at.timestamp_millis().div_euclid(width_ms) * width_ms;
        let starts_at = Utc
            .timestamp_millis_opt(starts_at_ms)
            .single()
            .ok_or(EventTimeWindowError::EventTimeOutOfRange)?;
        let ends_at = starts_at
            .checked_add_signed(self.config.window_size)
            .ok_or(EventTimeWindowError::EventTimeOutOfRange)?;
        Ok((
            WindowKey {
                correlation_key: correlation_key.to_owned(),
                starts_at_ms,
            },
            starts_at,
            ends_at,
        ))
    }

    fn window_end(
        &self,
        observed_at: DateTime<Utc>,
    ) -> Result<DateTime<Utc>, EventTimeWindowError> {
        self.window_for("", observed_at)
            .map(|(_, _, ends_at)| ends_at)
    }

    fn close_eligible_windows(&mut self) -> Vec<EventTimeWindow> {
        let Some(watermark) = self.watermark() else {
            return Vec::new();
        };
        let keys = self
            .windows
            .iter()
            .filter(|(_, window)| window.ends_at <= watermark)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let complete = self.windows[&key].is_complete(&self.config.sources);
                self.remove_window(&key, complete)
            })
            .collect()
    }

    fn remove_window(&mut self, key: &WindowKey, complete: bool) -> EventTimeWindow {
        let mut window = self
            .windows
            .remove(key)
            .expect("window key came from the active window map");
        for records in window.records.values_mut() {
            records.sort_by(|left, right| {
                (
                    left.observed_at,
                    &left.position.topic,
                    left.position.partition,
                    left.position.offset,
                    &left.event_id,
                )
                    .cmp(&(
                        right.observed_at,
                        &right.position.topic,
                        right.position.partition,
                        right.position.offset,
                        &right.event_id,
                    ))
            });
        }
        self.finalized.insert(key.clone(), window.ends_at);
        if complete {
            self.stats.complete_windows = self.stats.complete_windows.saturating_add(1);
        } else {
            self.stats.incomplete_windows = self.stats.incomplete_windows.saturating_add(1);
        }
        window
    }

    fn prune_replay_state(&mut self) {
        let Some(watermark) = self.watermark() else {
            return;
        };
        self.finalized.retain(|_, ends_at| *ends_at > watermark);
        let window_size = self.config.window_size;
        self.seen_event_ids.retain(|_, observed_at| {
            let width_ms = window_size.num_milliseconds();
            let starts_at_ms = observed_at.timestamp_millis().div_euclid(width_ms) * width_ms;
            Utc.timestamp_millis_opt(starts_at_ms)
                .single()
                .and_then(|starts_at| starts_at.checked_add_signed(window_size))
                .is_none_or(|ends_at| ends_at > watermark)
        });
    }

    fn validate_restored_state(&self) -> Result<(), EventTimeWindowError> {
        if self.windows.len() > self.config.max_open_windows {
            return invalid_snapshot("open-window capacity exceeded");
        }
        if self.seen_event_ids.len() > self.config.max_tracked_event_ids {
            return invalid_snapshot("tracked event-ID capacity exceeded");
        }
        if self
            .source_high_water
            .keys()
            .any(|lane| !self.config.watermark_lanes.contains(lane))
        {
            return invalid_snapshot("watermark contains an unknown lane");
        }
        if self
            .finalized
            .keys()
            .any(|key| self.windows.contains_key(key))
        {
            return invalid_snapshot("a window is both open and finalized");
        }
        for (key, ends_at) in &self.finalized {
            let Some(starts_at) = Utc.timestamp_millis_opt(key.starts_at_ms).single() else {
                return invalid_snapshot("a finalized window start is out of range");
            };
            if key.correlation_key.trim().is_empty()
                || starts_at.checked_add_signed(self.config.window_size) != Some(*ends_at)
            {
                return invalid_snapshot("a finalized window key or end is invalid");
            }
        }
        if self
            .seen_event_ids
            .keys()
            .any(|event_id| event_id.trim().is_empty())
        {
            return invalid_snapshot("the deduplication ledger contains an empty event ID");
        }
        let mut record_ids = BTreeSet::new();
        for (key, window) in &self.windows {
            self.validate_restored_window(key, window, &mut record_ids)?;
        }
        Ok(())
    }

    fn validate_restored_window(
        &self,
        key: &WindowKey,
        window: &EventTimeWindow,
        record_ids: &mut BTreeSet<String>,
    ) -> Result<(), EventTimeWindowError> {
        if key.correlation_key.trim().is_empty()
            || key.correlation_key != window.correlation_key
            || key.starts_at_ms != window.starts_at.timestamp_millis()
            || window.starts_at.checked_add_signed(self.config.window_size) != Some(window.ends_at)
        {
            return invalid_snapshot("window key or boundaries do not match its state");
        }
        if window.record_count() == 0
            || window.record_count() > self.config.max_records_per_window
            || window.records.values().any(Vec::is_empty)
            || window
                .records
                .values()
                .any(|records| records.len() > self.config.max_records_per_source_per_window)
            || window
                .records
                .keys()
                .any(|source| !self.config.sources.contains(source))
        {
            return invalid_snapshot("window source or record capacity is invalid");
        }
        if self.config.emit_when_complete && window.is_complete(&self.config.sources) {
            return invalid_snapshot("an emit-on-complete window is still open");
        }
        if self
            .watermark()
            .is_some_and(|watermark| window.ends_at <= watermark)
        {
            return invalid_snapshot("an open window is behind the watermark");
        }
        for (source, records) in &window.records {
            for record in records {
                if self.validate_record(record).is_err()
                    || self.watermark_lane(record).is_err()
                    || &record.source != source
                    || record.correlation_key != window.correlation_key
                    || !(window.starts_at..window.ends_at).contains(&record.observed_at)
                    || self.seen_event_ids.get(&record.event_id) != Some(&record.observed_at)
                    || !record_ids.insert(record.event_id.clone())
                {
                    return invalid_snapshot(
                        "window record does not match its source, key, time, or dedup state",
                    );
                }
            }
        }
        Ok(())
    }
}

fn collect_unique<K: Ord, V>(
    entries: Vec<(K, V)>,
    collection: &'static str,
) -> Result<BTreeMap<K, V>, EventTimeWindowError> {
    let count = entries.len();
    let values = entries.into_iter().collect::<BTreeMap<_, _>>();
    if values.len() != count {
        return Err(EventTimeWindowError::DuplicateSnapshotEntry { collection });
    }
    Ok(values)
}

fn invalid_snapshot<T>(message: &str) -> Result<T, EventTimeWindowError> {
    Err(EventTimeWindowError::InvalidSnapshot(message.to_owned()))
}

fn watermark_for(
    lanes: &BTreeSet<WindowWatermarkLane>,
    source_high_water: &BTreeMap<WindowWatermarkLane, DateTime<Utc>>,
    allowed_lateness: Duration,
) -> Option<DateTime<Utc>> {
    if lanes
        .iter()
        .any(|lane| !source_high_water.contains_key(lane))
    {
        return None;
    }
    lanes
        .iter()
        .filter_map(|lane| source_high_water.get(lane))
        .copied()
        .min()
        .and_then(|minimum| minimum.checked_sub_signed(allowed_lateness))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> EventTimeWindowConfig {
        EventTimeWindowConfig::new(
            ["metrics", "traces", "logs"],
            Duration::minutes(1),
            Duration::seconds(15),
        )
        .unwrap()
    }

    fn record(source: &str, id: &str, at: &str, offset: i64) -> WindowRecord {
        WindowRecord {
            event_id: id.to_owned(),
            source: source.to_owned(),
            correlation_key: "acme:prod:checkout".to_owned(),
            observed_at: at.parse().unwrap(),
            payload: json!({"source": source}),
            position: WindowSourcePosition {
                topic: format!("observability.acme.{source}"),
                partition: 0,
                offset,
            },
        }
    }

    #[test]
    fn correlates_out_of_order_sources_and_preserves_positions() {
        let mut windows = EventTimeWindowAggregator::new(config()).unwrap();
        let emitted = [
            record("logs", "logs-1", "2026-10-01T19:42:30Z", 7),
            record("metrics", "metrics-1", "2026-10-01T19:42:10Z", 8),
            record("traces", "traces-1", "2026-10-01T19:42:20Z", 9),
        ]
        .into_iter()
        .flat_map(|record| windows.ingest(record).unwrap().emitted)
        .collect::<Vec<_>>();

        assert_eq!(emitted.len(), 1);
        assert_eq!(
            emitted[0].starts_at.to_rfc3339(),
            "2026-10-01T19:42:00+00:00"
        );
        assert!(emitted[0].is_complete(&config().sources));
        assert_eq!(emitted[0].records_for("logs")[0].position.offset, 7);
        assert_eq!(windows.stats().complete_windows, 1);
    }

    #[test]
    fn rejects_replays_and_does_not_reopen_an_emitted_window() {
        let mut windows = EventTimeWindowAggregator::new(config()).unwrap();
        let metrics = record("metrics", "metrics-1", "2026-10-01T19:42:10Z", 1);
        windows.ingest(metrics.clone()).unwrap();
        assert_eq!(
            windows.ingest(metrics).unwrap().disposition,
            WindowIngestDisposition::Duplicate
        );
        windows
            .ingest(record("traces", "traces-1", "2026-10-01T19:42:20Z", 1))
            .unwrap();
        windows
            .ingest(record("logs", "logs-1", "2026-10-01T19:42:30Z", 1))
            .unwrap();

        let late = windows
            .ingest(record("metrics", "metrics-late", "2026-10-01T19:42:40Z", 2))
            .unwrap();
        assert_eq!(late.disposition, WindowIngestDisposition::Late);
        assert_eq!(windows.stats().duplicate_records, 1);
        assert_eq!(windows.stats().late_records, 1);
    }

    #[test]
    fn watermark_closes_incomplete_windows() {
        let mut windows = EventTimeWindowAggregator::new(config()).unwrap();
        windows
            .ingest(record("metrics", "metrics-old", "2026-10-01T19:40:10Z", 1))
            .unwrap();
        windows
            .ingest(record("traces", "traces-old", "2026-10-01T19:40:20Z", 1))
            .unwrap();
        windows
            .ingest(record("metrics", "metrics-new", "2026-10-01T19:42:20Z", 2))
            .unwrap();
        windows
            .ingest(record("traces", "traces-new", "2026-10-01T19:42:20Z", 2))
            .unwrap();
        let result = windows
            .ingest(record("logs", "logs-new", "2026-10-01T19:42:20Z", 2))
            .unwrap();
        let old = result
            .emitted
            .iter()
            .find(|window| window.starts_at.to_rfc3339().contains("19:40:00"))
            .unwrap();
        assert_eq!(old.missing_sources(&config().sources), vec!["logs"]);
        assert_eq!(windows.stats().incomplete_windows, 1);
    }

    #[test]
    fn an_idle_source_can_advance_its_watermark_without_a_synthetic_record() {
        let mut config = config();
        config.emit_when_complete = false;
        let mut windows = EventTimeWindowAggregator::new(config).unwrap();
        windows
            .ingest(record("metrics", "metrics-1", "2026-10-01T19:40:10Z", 1))
            .unwrap();
        windows
            .advance_source_watermark("metrics", "2026-10-01T19:42:00Z".parse().unwrap())
            .unwrap();
        windows
            .advance_source_watermark("traces", "2026-10-01T19:42:00Z".parse().unwrap())
            .unwrap();
        let emitted = windows
            .advance_source_watermark("logs", "2026-10-01T19:42:00Z".parse().unwrap())
            .unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].missing_sources(&windows.config.sources).len(), 2);
    }

    #[test]
    fn a_fast_partition_cannot_advance_past_an_unreported_partition() {
        let mut config = config();
        config.emit_when_complete = false;
        config
            .set_source_partitions(
                "metrics",
                [
                    ("observability.acme.metrics", 0),
                    ("observability.acme.metrics", 1),
                ],
            )
            .unwrap();
        let mut windows = EventTimeWindowAggregator::new(config).unwrap();
        windows
            .ingest(record("logs", "logs-old", "2026-10-01T19:40:10Z", 1))
            .unwrap();
        windows
            .ingest(record("logs", "logs-new", "2026-10-01T19:44:10Z", 2))
            .unwrap();
        windows
            .ingest(record("traces", "traces-new", "2026-10-01T19:44:10Z", 2))
            .unwrap();
        assert!(
            windows
                .ingest(record("metrics", "metrics-new", "2026-10-01T19:44:10Z", 2))
                .unwrap()
                .emitted
                .is_empty()
        );
        assert!(windows.watermark().is_none());

        let emitted = windows
            .advance_watermark(
                &WindowWatermarkLane::partition("metrics", "observability.acme.metrics", 1),
                "2026-10-01T19:42:00Z".parse().unwrap(),
            )
            .unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(
            emitted[0].starts_at.to_rfc3339(),
            "2026-10-01T19:40:00+00:00"
        );
    }

    #[test]
    fn snapshot_restores_partial_windows_and_deduplication_state() {
        let mut before = EventTimeWindowAggregator::new(config()).unwrap();
        let metrics = record("metrics", "metrics-1", "2026-10-01T19:42:10Z", 3);
        before.ingest(metrics.clone()).unwrap();
        before
            .ingest(record("traces", "traces-1", "2026-10-01T19:42:20Z", 4))
            .unwrap();

        let encoded = serde_json::to_vec(&before.snapshot()).unwrap();
        let snapshot = serde_json::from_slice(&encoded).unwrap();
        let mut after = EventTimeWindowAggregator::restore(snapshot).unwrap();
        assert_eq!(
            after.ingest(metrics).unwrap().disposition,
            WindowIngestDisposition::Duplicate
        );
        let result = after
            .ingest(record("logs", "logs-1", "2026-10-01T19:42:30Z", 5))
            .unwrap();
        assert_eq!(result.emitted.len(), 1);
        assert!(result.emitted[0].is_complete(&after.config.sources));
    }

    #[test]
    fn one_source_can_contribute_multiple_records_until_the_watermark() {
        let mut config = config();
        config.emit_when_complete = false;
        let mut windows = EventTimeWindowAggregator::new(config).unwrap();
        windows
            .ingest(record("logs", "logs-2", "2026-10-01T19:42:20Z", 2))
            .unwrap();
        windows
            .ingest(record("logs", "logs-1", "2026-10-01T19:42:10Z", 1))
            .unwrap();
        windows
            .advance_source_watermark("metrics", "2026-10-01T19:44:00Z".parse().unwrap())
            .unwrap();
        windows
            .advance_source_watermark("traces", "2026-10-01T19:44:00Z".parse().unwrap())
            .unwrap();
        let emitted = windows
            .advance_source_watermark("logs", "2026-10-01T19:44:00Z".parse().unwrap())
            .unwrap();

        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].records_for("logs").len(), 2);
        assert_eq!(emitted[0].records_for("logs")[0].event_id, "logs-1");
        assert_eq!(
            emitted[0].idempotency_key(),
            "18:acme:prod:checkout:1790883720000"
        );
    }

    #[test]
    fn restore_rejects_corrupted_window_boundaries_and_duplicate_entries() {
        let mut windows = EventTimeWindowAggregator::new(config()).unwrap();
        windows
            .ingest(record("metrics", "metrics-1", "2026-10-01T19:42:10Z", 1))
            .unwrap();
        let mut invalid_boundary = windows.snapshot();
        invalid_boundary.windows[0].1.ends_at += Duration::seconds(1);
        assert!(matches!(
            EventTimeWindowAggregator::restore(invalid_boundary),
            Err(EventTimeWindowError::InvalidSnapshot(_))
        ));

        let mut duplicate = windows.snapshot();
        duplicate.windows.push(duplicate.windows[0].clone());
        assert!(matches!(
            EventTimeWindowAggregator::restore(duplicate),
            Err(EventTimeWindowError::DuplicateSnapshotEntry {
                collection: "window"
            })
        ));

        let mut invalid_position = windows.snapshot();
        invalid_position.windows[0]
            .1
            .records
            .get_mut("metrics")
            .unwrap()[0]
            .position
            .topic
            .clear();
        assert!(matches!(
            EventTimeWindowAggregator::restore(invalid_position),
            Err(EventTimeWindowError::InvalidSnapshot(_))
        ));
    }

    #[test]
    fn bus_normalization_requires_a_consumed_position() {
        let message = BusMessage::new("observability.acme.metrics", json!({}));
        let error = WindowRecord::from_bus_message(
            message,
            "event-1",
            "metrics",
            "acme:prod:checkout",
            "2026-10-01T19:42:10Z".parse().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EventTimeWindowError::MissingPosition {
                field: "partition",
                ..
            }
        ));
    }

    #[test]
    fn capacity_limits_fail_before_admitting_the_record() {
        let mut limited = config();
        limited.max_open_windows = 1;
        limited.max_records_per_window = 1;
        let mut windows = EventTimeWindowAggregator::new(limited).unwrap();
        windows
            .ingest(record("metrics", "first", "2026-10-01T19:40:10Z", 1))
            .unwrap();
        let record_limit = windows
            .ingest(record("metrics", "second", "2026-10-01T19:40:20Z", 2))
            .unwrap_err();
        assert!(matches!(
            record_limit,
            EventTimeWindowError::WindowRecordCapacity { .. }
        ));
        let window_limit = windows
            .ingest(record("metrics", "third", "2026-10-01T19:41:20Z", 3))
            .unwrap_err();
        assert!(matches!(
            window_limit,
            EventTimeWindowError::OpenWindowCapacity { .. }
        ));
        assert_eq!(windows.stats().accepted_records, 1);

        let mut per_source = config();
        per_source.max_records_per_source_per_window = 1;
        let mut windows = EventTimeWindowAggregator::new(per_source).unwrap();
        windows
            .ingest(record("metrics", "source-first", "2026-10-01T19:40:10Z", 1))
            .unwrap();
        let source_limit = windows
            .ingest(record(
                "metrics",
                "source-second",
                "2026-10-01T19:40:20Z",
                2,
            ))
            .unwrap_err();
        assert!(matches!(
            source_limit,
            EventTimeWindowError::SourceRecordCapacity { .. }
        ));
        assert_eq!(windows.stats().accepted_records, 1);
    }
}
