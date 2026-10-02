use std::collections::BTreeMap;

use acteon_bus::{
    BusMessage, EventTimeWindow, EventTimeWindowAggregator, EventTimeWindowConfig,
    EventTimeWindowError, EventTimeWindowSnapshot, WindowIngestDisposition, WindowRecord,
    WindowSourcePosition,
};
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

    fn parse(value: &str) -> Result<Self, WindowingError> {
        match value {
            "metrics" => Ok(Self::Metrics),
            "traces" => Ok(Self::Traces),
            "logs" => Ok(Self::Logs),
            other => Err(WindowingError::UnexpectedSource(other.to_owned())),
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

pub type SourcePosition = WindowSourcePosition;
pub type CorrelatorSnapshot = EventTimeWindowSnapshot;

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

impl From<WindowIngestDisposition> for IngestDisposition {
    fn from(value: WindowIngestDisposition) -> Self {
        match value {
            WindowIngestDisposition::Accepted => Self::Accepted,
            WindowIngestDisposition::Duplicate => Self::Duplicate,
            WindowIngestDisposition::Late => Self::Late,
        }
    }
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
    #[error(transparent)]
    Platform(#[from] EventTimeWindowError),
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
    #[error("platform window contained unexpected source '{0}'")]
    UnexpectedSource(String),
    #[error("platform window contained {count} records for source '{signal_source}'")]
    SourceCardinality { signal_source: String, count: usize },
    #[error("platform window did not contain any telemetry records")]
    EmptyWindow,
    #[error("platform counter {field} cannot fit in usize")]
    CounterOverflow { field: &'static str },
}

/// Scenario-specific schema validation around Acteon's generic window operator.
///
/// The source list, watermarking, deduplication, capacity bounds, snapshots,
/// and broker-position preservation all live in `acteon-bus`.
pub struct EventTimeCorrelator {
    inner: EventTimeWindowAggregator,
    stats: WindowingStats,
}

impl EventTimeCorrelator {
    pub fn new(window_size: Duration, allowed_lateness: Duration) -> Result<Self, WindowingError> {
        let mut config = EventTimeWindowConfig::new(
            SignalSource::ALL.map(|source| source.as_str()),
            window_size,
            allowed_lateness,
        )?;
        // This scenario publishes exactly one aggregate from each signal
        // source. Reject a conflicting second aggregate before mutation.
        config.max_records_per_source_per_window = 1;
        let inner = EventTimeWindowAggregator::new(config)?;
        Ok(Self {
            inner,
            stats: WindowingStats::default(),
        })
    }

    pub fn ingest(&mut self, message: BusMessage) -> Result<IngestResult, WindowingError> {
        let message_key = message.key.clone();
        let event: TelemetryEvent = serde_json::from_value(message.payload.clone())?;
        self.validate_event(&event, message_key.as_deref())?;
        let record = WindowRecord::from_bus_message(
            message,
            event.event_id.clone(),
            event.source.as_str(),
            event.correlation_key(),
            event.observed_at,
        )?;
        let result = self.inner.ingest(record)?;
        self.refresh_stats()?;
        Ok(IngestResult {
            disposition: result.disposition.into(),
            emitted: result
                .emitted
                .into_iter()
                .map(Self::convert_window)
                .collect::<Result<_, _>>()?,
        })
    }

    pub fn finish(&mut self) -> Result<Vec<CorrelatedWindow>, WindowingError> {
        let windows = self
            .inner
            .finish()
            .into_iter()
            .map(Self::convert_window)
            .collect::<Result<_, _>>()?;
        self.refresh_stats()?;
        Ok(windows)
    }

    pub fn stats(&self) -> &WindowingStats {
        &self.stats
    }

    pub fn snapshot(&self) -> CorrelatorSnapshot {
        self.inner.snapshot()
    }

    pub fn restore(snapshot: CorrelatorSnapshot) -> Result<Self, WindowingError> {
        let inner = EventTimeWindowAggregator::restore(snapshot)?;
        let mut restored = Self {
            inner,
            stats: WindowingStats::default(),
        };
        restored.refresh_stats()?;
        Ok(restored)
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
        let width_ms = self.inner.config().window_size.num_milliseconds();
        let starts_at_ms = event.observed_at.timestamp_millis().div_euclid(width_ms) * width_ms;
        let starts_at = Utc
            .timestamp_millis_opt(starts_at_ms)
            .single()
            .ok_or(EventTimeWindowError::EventTimeOutOfRange)?;
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

    fn convert_window(window: EventTimeWindow) -> Result<CorrelatedWindow, WindowingError> {
        let mut signals = BTreeMap::new();
        let mut envelope = None;
        for (source_name, records) in window.records {
            if records.len() != 1 {
                return Err(WindowingError::SourceCardinality {
                    signal_source: source_name,
                    count: records.len(),
                });
            }
            let source = SignalSource::parse(&source_name)?;
            let record = records.into_iter().next().expect("length checked above");
            let event: TelemetryEvent = serde_json::from_value(record.payload)?;
            envelope.get_or_insert_with(|| event.clone());
            signals.insert(
                source,
                SourceRecord {
                    event_id: event.event_id,
                    observed_at: event.observed_at,
                    ingested_at: event.ingested_at,
                    available: event.available,
                    features: event.features,
                    position: record.position,
                },
            );
        }
        let envelope = envelope.ok_or(WindowingError::EmptyWindow)?;
        Ok(CorrelatedWindow {
            window_id: envelope.window_id,
            tenant: envelope.tenant,
            environment: envelope.environment,
            service: envelope.service,
            deployment_revision: envelope.deployment_revision,
            starts_at: window.starts_at,
            ends_at: window.ends_at,
            signals,
        })
    }

    fn refresh_stats(&mut self) -> Result<(), WindowingError> {
        let platform = self.inner.stats();
        self.stats = WindowingStats {
            accepted_records: counter(platform.accepted_records, "accepted_records")?,
            duplicate_records: counter(platform.duplicate_records, "duplicate_records")?,
            late_records: counter(platform.late_records, "late_records")?,
            complete_windows: counter(platform.complete_windows, "complete_windows")?,
            incomplete_windows: counter(platform.incomplete_windows, "incomplete_windows")?,
        };
        Ok(())
    }
}

fn counter(value: u64, field: &'static str) -> Result<usize, WindowingError> {
    usize::try_from(value).map_err(|_| WindowingError::CounterOverflow { field })
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
    fn scenario_adapter_uses_generic_windows_and_preserves_positions() {
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
        assert_eq!(
            windows[0]
                .signal(SignalSource::Logs)
                .unwrap()
                .position
                .offset,
            7
        );
    }

    #[test]
    fn snapshot_restores_scenario_deduplication_state() {
        let mut before = correlator();
        let metrics = message(
            SignalSource::Metrics,
            "metrics-1",
            "2026-10-01T19:42:10Z",
            3,
        );
        before.ingest(metrics.clone()).unwrap();
        let encoded = serde_json::to_vec(&before.snapshot()).unwrap();
        let snapshot = serde_json::from_slice(&encoded).unwrap();
        let mut after = EventTimeCorrelator::restore(snapshot).unwrap();
        assert_eq!(
            after.ingest(metrics).unwrap().disposition,
            IngestDisposition::Duplicate
        );
    }

    #[test]
    fn scenario_rejects_a_second_aggregate_from_one_source_before_mutation() {
        let mut correlator = correlator();
        correlator
            .ingest(message(
                SignalSource::Metrics,
                "metrics-1",
                "2026-10-01T19:42:10Z",
                1,
            ))
            .unwrap();
        let error = correlator
            .ingest(message(
                SignalSource::Metrics,
                "metrics-2",
                "2026-10-01T19:42:20Z",
                2,
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            WindowingError::Platform(EventTimeWindowError::SourceRecordCapacity { .. })
        ));
        assert_eq!(correlator.stats().accepted_records, 1);
    }
}
