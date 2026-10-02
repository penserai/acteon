use std::collections::{BTreeMap, BTreeSet};

use acteon_bus::BusMessage;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalSource {
    Metrics,
    Traces,
    Logs,
}

impl SignalSource {
    pub const ALL: [Self; 3] = [Self::Metrics, Self::Traces, Self::Logs];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Traces => "traces",
            Self::Logs => "logs",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryEvent {
    pub schema_version: u16,
    pub event_id: String,
    pub observed_at: DateTime<Utc>,
    pub ingested_at: DateTime<Utc>,
    pub window_id: String,
    pub tenant: String,
    pub environment: String,
    pub service: String,
    pub deployment_revision: String,
    pub source: SignalSource,
    pub available: bool,
    pub features: Value,
}

impl TelemetryEvent {
    pub fn correlation_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.tenant, self.environment, self.service, self.deployment_revision
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePosition {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub event_id: String,
    pub observed_at: DateTime<Utc>,
    pub ingested_at: DateTime<Utc>,
    pub available: bool,
    pub features: Value,
    pub position: SourcePosition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelatedWindow {
    pub window_id: String,
    pub tenant: String,
    pub environment: String,
    pub service: String,
    pub deployment_revision: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub signals: BTreeMap<SignalSource, SourceRecord>,
}

impl CorrelatedWindow {
    pub fn signal(&self, source: SignalSource) -> Option<&SourceRecord> {
        self.signals.get(&source)
    }

    pub fn missing_sources(&self) -> Vec<SignalSource> {
        SignalSource::ALL
            .into_iter()
            .filter(|source| {
                self.signals
                    .get(source)
                    .is_none_or(|record| !record.available)
            })
            .collect()
    }

    pub fn has_all_source_records(&self) -> bool {
        SignalSource::ALL
            .into_iter()
            .all(|source| self.signals.contains_key(&source))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestDisposition {
    Accepted,
    Duplicate,
    Late,
}

#[derive(Debug)]
pub struct IngestResult {
    pub disposition: IngestDisposition,
    pub emitted: Vec<CorrelatedWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowingStats {
    pub accepted_records: usize,
    pub duplicate_records: usize,
    pub late_records: usize,
    pub complete_windows: usize,
    pub incomplete_windows: usize,
}

#[derive(Debug, Error)]
pub enum WindowingError {
    #[error("window size must be positive")]
    InvalidWindowSize,
    #[error("message {topic} has no broker {field}")]
    MissingPosition { topic: String, field: &'static str },
    #[error("invalid telemetry envelope: {0}")]
    InvalidEnvelope(#[from] serde_json::Error),
    #[error("unsupported telemetry schema version {0}")]
    UnsupportedSchema(u16),
    #[error("message key {actual:?} does not match correlation key {expected}")]
    CorrelationKeyMismatch {
        expected: String,
        actual: Option<String>,
    },
    #[error("window ID {actual} does not match event-time window {expected}")]
    WindowIdMismatch { expected: String, actual: String },
    #[error("source {signal_source:?} already has a different aggregate for window {window_id}")]
    ConflictingSource {
        signal_source: SignalSource,
        window_id: String,
    },
    #[error("event time is outside chrono's supported range")]
    EventTimeOutOfRange,
    #[error("unsupported correlator snapshot version {0}")]
    UnsupportedSnapshotVersion(u16),
    #[error("correlator snapshot has an invalid {field}: {value}")]
    InvalidSnapshotDuration { field: &'static str, value: i64 },
    #[error("correlator snapshot contains duplicate {collection} entry")]
    DuplicateSnapshotEntry { collection: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
struct WindowKey {
    tenant: String,
    environment: String,
    service: String,
    deployment_revision: String,
    starts_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct WindowState {
    window_id: String,
    tenant: String,
    environment: String,
    service: String,
    deployment_revision: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    signals: BTreeMap<SignalSource, SourceRecord>,
}

impl WindowState {
    fn finish(self) -> CorrelatedWindow {
        CorrelatedWindow {
            window_id: self.window_id,
            tenant: self.tenant,
            environment: self.environment,
            service: self.service,
            deployment_revision: self.deployment_revision,
            starts_at: self.starts_at,
            ends_at: self.ends_at,
            signals: self.signals,
        }
    }
}

pub struct EventTimeCorrelator {
    window_size: Duration,
    allowed_lateness: Duration,
    windows: BTreeMap<WindowKey, WindowState>,
    finalized: BTreeSet<WindowKey>,
    seen_event_ids: BTreeSet<String>,
    source_high_water: BTreeMap<SignalSource, DateTime<Utc>>,
    stats: WindowingStats,
}

const SNAPSHOT_VERSION: u16 = 1;

/// Versioned, deterministic representation of all event-time state required
/// to resume after a process restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrelatorSnapshot {
    schema_version: u16,
    window_size_ms: i64,
    allowed_lateness_ms: i64,
    windows: Vec<(WindowKey, WindowState)>,
    finalized: Vec<WindowKey>,
    seen_event_ids: Vec<String>,
    source_high_water: BTreeMap<SignalSource, DateTime<Utc>>,
    stats: WindowingStats,
}

impl EventTimeCorrelator {
    pub fn new(window_size: Duration, allowed_lateness: Duration) -> Result<Self, WindowingError> {
        if window_size <= Duration::zero() {
            return Err(WindowingError::InvalidWindowSize);
        }
        Ok(Self {
            window_size,
            allowed_lateness,
            windows: BTreeMap::new(),
            finalized: BTreeSet::new(),
            seen_event_ids: BTreeSet::new(),
            source_high_water: BTreeMap::new(),
            stats: WindowingStats::default(),
        })
    }

    pub fn ingest(&mut self, message: BusMessage) -> Result<IngestResult, WindowingError> {
        let topic = message.topic.clone();
        let partition = message
            .partition
            .ok_or_else(|| WindowingError::MissingPosition {
                topic: topic.clone(),
                field: "partition",
            })?;
        let offset = message
            .offset
            .ok_or_else(|| WindowingError::MissingPosition {
                topic: topic.clone(),
                field: "offset",
            })?;
        let event: TelemetryEvent = serde_json::from_value(message.payload)?;
        self.validate_event(&event, message.key.as_deref())?;

        if !self.seen_event_ids.insert(event.event_id.clone()) {
            self.stats.duplicate_records += 1;
            return Ok(IngestResult {
                disposition: IngestDisposition::Duplicate,
                emitted: Vec::new(),
            });
        }

        self.source_high_water
            .entry(event.source)
            .and_modify(|high_water| *high_water = (*high_water).max(event.observed_at))
            .or_insert(event.observed_at);

        let (key, starts_at, ends_at) = self.window_key(&event)?;
        if self.finalized.contains(&key)
            || self
                .watermark()
                .is_some_and(|watermark| ends_at <= watermark)
        {
            self.stats.late_records += 1;
            return Ok(IngestResult {
                disposition: IngestDisposition::Late,
                emitted: self.close_eligible_windows(),
            });
        }

        let state = self
            .windows
            .entry(key.clone())
            .or_insert_with(|| WindowState {
                window_id: event.window_id.clone(),
                tenant: event.tenant.clone(),
                environment: event.environment.clone(),
                service: event.service.clone(),
                deployment_revision: event.deployment_revision.clone(),
                starts_at,
                ends_at,
                signals: BTreeMap::new(),
            });
        if state.signals.contains_key(&event.source) {
            return Err(WindowingError::ConflictingSource {
                signal_source: event.source,
                window_id: state.window_id.clone(),
            });
        }
        state.signals.insert(
            event.source,
            SourceRecord {
                event_id: event.event_id,
                observed_at: event.observed_at,
                ingested_at: event.ingested_at,
                available: event.available,
                features: event.features,
                position: SourcePosition {
                    topic,
                    partition,
                    offset,
                },
            },
        );
        self.stats.accepted_records += 1;

        let mut emitted = Vec::new();
        if state.signals.len() == SignalSource::ALL.len() {
            emitted.push(self.remove_window(&key, true));
        }
        emitted.extend(self.close_eligible_windows());
        Ok(IngestResult {
            disposition: IngestDisposition::Accepted,
            emitted,
        })
    }

    pub fn finish(&mut self) -> Vec<CorrelatedWindow> {
        let keys = self.windows.keys().cloned().collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let complete = self.windows[&key].signals.len() == SignalSource::ALL.len();
                self.remove_window(&key, complete)
            })
            .collect()
    }

    pub fn stats(&self) -> &WindowingStats {
        &self.stats
    }

    pub fn snapshot(&self) -> CorrelatorSnapshot {
        CorrelatorSnapshot {
            schema_version: SNAPSHOT_VERSION,
            window_size_ms: self.window_size.num_milliseconds(),
            allowed_lateness_ms: self.allowed_lateness.num_milliseconds(),
            windows: self
                .windows
                .iter()
                .map(|(key, state)| (key.clone(), state.clone()))
                .collect(),
            finalized: self.finalized.iter().cloned().collect(),
            seen_event_ids: self.seen_event_ids.iter().cloned().collect(),
            source_high_water: self.source_high_water.clone(),
            stats: self.stats.clone(),
        }
    }

    pub fn restore(snapshot: CorrelatorSnapshot) -> Result<Self, WindowingError> {
        if snapshot.schema_version != SNAPSHOT_VERSION {
            return Err(WindowingError::UnsupportedSnapshotVersion(
                snapshot.schema_version,
            ));
        }
        if snapshot.window_size_ms <= 0 {
            return Err(WindowingError::InvalidSnapshotDuration {
                field: "window_size_ms",
                value: snapshot.window_size_ms,
            });
        }
        if snapshot.allowed_lateness_ms < 0 {
            return Err(WindowingError::InvalidSnapshotDuration {
                field: "allowed_lateness_ms",
                value: snapshot.allowed_lateness_ms,
            });
        }

        let window_count = snapshot.windows.len();
        let windows = snapshot.windows.into_iter().collect::<BTreeMap<_, _>>();
        if windows.len() != window_count {
            return Err(WindowingError::DuplicateSnapshotEntry {
                collection: "window",
            });
        }
        let finalized_count = snapshot.finalized.len();
        let finalized = snapshot.finalized.into_iter().collect::<BTreeSet<_>>();
        if finalized.len() != finalized_count {
            return Err(WindowingError::DuplicateSnapshotEntry {
                collection: "finalized window",
            });
        }
        let seen_count = snapshot.seen_event_ids.len();
        let seen_event_ids = snapshot.seen_event_ids.into_iter().collect::<BTreeSet<_>>();
        if seen_event_ids.len() != seen_count {
            return Err(WindowingError::DuplicateSnapshotEntry {
                collection: "event ID",
            });
        }

        Ok(Self {
            window_size: Duration::milliseconds(snapshot.window_size_ms),
            allowed_lateness: Duration::milliseconds(snapshot.allowed_lateness_ms),
            windows,
            finalized,
            seen_event_ids,
            source_high_water: snapshot.source_high_water,
            stats: snapshot.stats,
        })
    }

    pub fn watermark(&self) -> Option<DateTime<Utc>> {
        if self.source_high_water.len() != SignalSource::ALL.len() {
            return None;
        }
        self.source_high_water
            .values()
            .copied()
            .min()
            .map(|minimum| minimum - self.allowed_lateness)
    }

    fn validate_event(
        &self,
        event: &TelemetryEvent,
        message_key: Option<&str>,
    ) -> Result<(), WindowingError> {
        if event.schema_version != SCHEMA_VERSION {
            return Err(WindowingError::UnsupportedSchema(event.schema_version));
        }
        let expected_key = event.correlation_key();
        if message_key != Some(expected_key.as_str()) {
            return Err(WindowingError::CorrelationKeyMismatch {
                expected: expected_key,
                actual: message_key.map(str::to_owned),
            });
        }
        let (_, starts_at, _) = self.window_key(event)?;
        let expected_window_id = format!(
            "{}:{}:{}",
            event.service,
            event.environment,
            starts_at.format("%Y-%m-%dT%H:%MZ")
        );
        if event.window_id != expected_window_id {
            return Err(WindowingError::WindowIdMismatch {
                expected: expected_window_id,
                actual: event.window_id.clone(),
            });
        }
        Ok(())
    }

    fn window_key(
        &self,
        event: &TelemetryEvent,
    ) -> Result<(WindowKey, DateTime<Utc>, DateTime<Utc>), WindowingError> {
        let width_ms = self.window_size.num_milliseconds();
        let starts_at_ms = event.observed_at.timestamp_millis().div_euclid(width_ms) * width_ms;
        let starts_at = Utc
            .timestamp_millis_opt(starts_at_ms)
            .single()
            .ok_or(WindowingError::EventTimeOutOfRange)?;
        let ends_at = starts_at + self.window_size;
        Ok((
            WindowKey {
                tenant: event.tenant.clone(),
                environment: event.environment.clone(),
                service: event.service.clone(),
                deployment_revision: event.deployment_revision.clone(),
                starts_at_ms,
            },
            starts_at,
            ends_at,
        ))
    }

    fn close_eligible_windows(&mut self) -> Vec<CorrelatedWindow> {
        let Some(watermark) = self.watermark() else {
            return Vec::new();
        };
        let keys = self
            .windows
            .iter()
            .filter(|(_, state)| state.ends_at <= watermark)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.into_iter()
            .map(|key| {
                let complete = self.windows[&key].signals.len() == SignalSource::ALL.len();
                self.remove_window(&key, complete)
            })
            .collect()
    }

    fn remove_window(&mut self, key: &WindowKey, complete: bool) -> CorrelatedWindow {
        let state = self
            .windows
            .remove(key)
            .expect("window key came from the active window map");
        self.finalized.insert(key.clone());
        if complete {
            self.stats.complete_windows += 1;
        } else {
            self.stats.incomplete_windows += 1;
        }
        state.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(source: SignalSource, event_id: &str, observed_at: &str, offset: i64) -> BusMessage {
        let observed_at = observed_at.parse::<DateTime<Utc>>().unwrap();
        let window_start_ms = observed_at.timestamp_millis().div_euclid(60_000) * 60_000;
        let window_start = Utc.timestamp_millis_opt(window_start_ms).unwrap();
        let event = TelemetryEvent {
            schema_version: SCHEMA_VERSION,
            event_id: event_id.to_owned(),
            observed_at,
            ingested_at: observed_at + Duration::seconds(1),
            window_id: format!(
                "checkout-api:prod:{}",
                window_start.format("%Y-%m-%dT%H:%MZ")
            ),
            tenant: "acme".to_owned(),
            environment: "prod".to_owned(),
            service: "checkout-api".to_owned(),
            deployment_revision: "checkout-v42".to_owned(),
            source,
            available: true,
            features: json!({"source": source.as_str()}),
        };
        BusMessage {
            topic: format!("observability.acme.{}", source.as_str()),
            key: Some(event.correlation_key()),
            payload: serde_json::to_value(event).unwrap(),
            headers: BTreeMap::new(),
            partition: Some(0),
            offset: Some(offset),
            timestamp: Some(observed_at),
        }
    }

    fn correlator() -> EventTimeCorrelator {
        EventTimeCorrelator::new(Duration::minutes(1), Duration::seconds(15)).unwrap()
    }

    #[test]
    fn correlates_out_of_order_sources_and_preserves_positions() {
        let mut correlator = correlator();
        let inputs = [
            message(SignalSource::Logs, "logs-1", "2026-10-01T19:42:30Z", 7),
            message(
                SignalSource::Metrics,
                "metrics-1",
                "2026-10-01T19:42:10Z",
                8,
            ),
            message(SignalSource::Traces, "traces-1", "2026-10-01T19:42:20Z", 9),
        ];
        let windows = inputs
            .into_iter()
            .flat_map(|message| correlator.ingest(message).unwrap().emitted)
            .collect::<Vec<_>>();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_id, "checkout-api:prod:2026-10-01T19:42Z");
        assert!(windows[0].has_all_source_records());
        assert_eq!(
            windows[0]
                .signal(SignalSource::Logs)
                .unwrap()
                .position
                .offset,
            7
        );
        assert_eq!(correlator.stats().complete_windows, 1);
    }

    #[test]
    fn rejects_duplicate_and_late_records_without_reopening_a_window() {
        let mut correlator = correlator();
        let metrics = message(
            SignalSource::Metrics,
            "metrics-1",
            "2026-10-01T19:42:10Z",
            1,
        );
        correlator.ingest(metrics.clone()).unwrap();
        assert_eq!(
            correlator.ingest(metrics).unwrap().disposition,
            IngestDisposition::Duplicate
        );
        correlator
            .ingest(message(
                SignalSource::Traces,
                "traces-1",
                "2026-10-01T19:42:20Z",
                1,
            ))
            .unwrap();
        correlator
            .ingest(message(
                SignalSource::Logs,
                "logs-1",
                "2026-10-01T19:42:30Z",
                1,
            ))
            .unwrap();
        let late = correlator
            .ingest(message(
                SignalSource::Metrics,
                "metrics-late",
                "2026-10-01T19:42:40Z",
                2,
            ))
            .unwrap();
        assert_eq!(late.disposition, IngestDisposition::Late);
        assert_eq!(correlator.stats().duplicate_records, 1);
        assert_eq!(correlator.stats().late_records, 1);
    }

    #[test]
    fn watermark_closes_an_incomplete_window_with_a_missing_source_mask() {
        let mut correlator = correlator();
        correlator
            .ingest(message(
                SignalSource::Metrics,
                "metrics-old",
                "2026-10-01T19:40:10Z",
                1,
            ))
            .unwrap();
        correlator
            .ingest(message(
                SignalSource::Traces,
                "traces-old",
                "2026-10-01T19:40:20Z",
                1,
            ))
            .unwrap();
        correlator
            .ingest(message(
                SignalSource::Metrics,
                "metrics-new",
                "2026-10-01T19:42:20Z",
                2,
            ))
            .unwrap();
        correlator
            .ingest(message(
                SignalSource::Traces,
                "traces-new",
                "2026-10-01T19:42:20Z",
                2,
            ))
            .unwrap();
        let result = correlator
            .ingest(message(
                SignalSource::Logs,
                "logs-new",
                "2026-10-01T19:42:20Z",
                2,
            ))
            .unwrap();
        let old = result
            .emitted
            .iter()
            .find(|window| window.window_id.ends_with("19:40Z"))
            .unwrap();
        assert_eq!(old.missing_sources(), vec![SignalSource::Logs]);
        assert_eq!(correlator.stats().incomplete_windows, 1);
    }

    #[test]
    fn snapshot_restores_partial_windows_and_deduplication_state() {
        let mut before_restart = correlator();
        let metrics = message(
            SignalSource::Metrics,
            "metrics-1",
            "2026-10-01T19:42:10Z",
            3,
        );
        let traces = message(SignalSource::Traces, "traces-1", "2026-10-01T19:42:20Z", 4);
        before_restart.ingest(metrics.clone()).unwrap();
        before_restart.ingest(traces).unwrap();

        let encoded = serde_json::to_vec(&before_restart.snapshot()).unwrap();
        let snapshot = serde_json::from_slice(&encoded).unwrap();
        let mut after_restart = EventTimeCorrelator::restore(snapshot).unwrap();

        assert_eq!(
            after_restart.ingest(metrics).unwrap().disposition,
            IngestDisposition::Duplicate
        );
        let result = after_restart
            .ingest(message(
                SignalSource::Logs,
                "logs-1",
                "2026-10-01T19:42:30Z",
                5,
            ))
            .unwrap();
        assert_eq!(result.emitted.len(), 1);
        assert!(result.emitted[0].has_all_source_records());
        assert_eq!(after_restart.stats().accepted_records, 3);
        assert_eq!(after_restart.stats().duplicate_records, 1);
        assert_eq!(after_restart.stats().complete_windows, 1);
    }
}
