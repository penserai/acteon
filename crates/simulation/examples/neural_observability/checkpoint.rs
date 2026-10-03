//! Telemetry-to-platform checkpoint conversion; no storage protocol lives here.
use std::collections::BTreeMap;
use std::sync::Arc;

use acteon_bus::{
    StreamCheckpointConfig, StreamCheckpointCoordinator, StreamOutboxEntry, StreamPosition,
    stream_checkpoint_key,
};
use acteon_state_redis::{RedisConfig, RedisStateStore};
use chrono::Utc;

use super::windowing::{CorrelatedWindow, CorrelatorSnapshot, SignalSource, SourcePosition};
use super::{AnyError, NAMESPACE, TENANT};

pub type WindowCheckpoint = StreamCheckpointCoordinator<CorrelatorSnapshot, CorrelatedWindow>;

pub async fn open_windows(
    config: &RedisConfig,
    initial: CorrelatorSnapshot,
) -> Result<WindowCheckpoint, AnyError> {
    Ok(StreamCheckpointCoordinator::initialize(
        Arc::new(RedisStateStore::new(config)?),
        stream_checkpoint_key(NAMESPACE, TENANT, "windows"),
        initial,
        StreamCheckpointConfig::default(),
    )
    .await?)
}

#[cfg(test)]
pub fn stream_positions(
    offsets: &BTreeMap<SignalSource, SourcePosition>,
    groups: &BTreeMap<SignalSource, String>,
) -> Vec<StreamPosition> {
    offsets
        .iter()
        .map(|(source, position)| StreamPosition {
            lane: acteon_bus::StreamPositionLane {
                source: source.as_str().into(),
                consumer_group: groups[source].clone(),
                topic: position.topic.clone(),
                partition: position.partition,
            },
            offset: position.offset,
        })
        .collect()
}

pub fn source_positions(
    positions: &[StreamPosition],
) -> Result<BTreeMap<SignalSource, SourcePosition>, AnyError> {
    positions
        .iter()
        .map(|position| {
            Ok((
                serde_json::from_value(serde_json::Value::String(position.lane.source.clone()))?,
                SourcePosition {
                    topic: position.lane.topic.clone(),
                    partition: position.lane.partition,
                    offset: position.offset,
                },
            ))
        })
        .collect()
}

pub fn window_key(window: &CorrelatedWindow) -> String {
    format!("window:{}", window.window_id)
}

pub fn window_outputs(
    windows: impl IntoIterator<Item = CorrelatedWindow>,
) -> Vec<StreamOutboxEntry<CorrelatedWindow>> {
    windows
        .into_iter()
        .map(|window| StreamOutboxEntry {
            idempotency_key: window_key(&window),
            created_at: Utc::now(),
            payload: window,
        })
        .collect()
}

/// Reject broker positions already represented by a restored checkpoint.
#[cfg(test)]
pub fn is_recovery_record(
    source: SignalSource,
    position: &SourcePosition,
    stored: &BTreeMap<SignalSource, SourcePosition>,
) -> bool {
    stored.get(&source).is_some_and(|checkpoint| {
        checkpoint.topic == position.topic
            && checkpoint.partition == position.partition
            && position.offset <= checkpoint.offset
    })
}
