use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::windowing::{CorrelatedWindow, CorrelatorSnapshot, SignalSource, SourcePosition};

const CHECKPOINT_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCheckpoint {
    pub schema_version: u16,
    pub generation: u64,
    pub correlator: CorrelatorSnapshot,
    pub ready_windows: Vec<CorrelatedWindow>,
    pub source_offsets: BTreeMap<SignalSource, SourcePosition>,
}

impl RecoveryCheckpoint {
    pub fn new(
        generation: u64,
        correlator: CorrelatorSnapshot,
        ready_windows: Vec<CorrelatedWindow>,
        source_offsets: BTreeMap<SignalSource, SourcePosition>,
    ) -> Self {
        Self {
            schema_version: CHECKPOINT_VERSION,
            generation,
            correlator,
            ready_windows,
            source_offsets,
        }
    }
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported recovery checkpoint version {0}")]
    UnsupportedVersion(u16),
    #[error("missing Kafka consumer group for {0:?}")]
    MissingConsumerGroup(SignalSource),
    #[error("offset commit failed after durable checkpoint: {0}")]
    Commit(String),
}

/// A single-file checkpoint store. Writes use fsync + atomic rename so a
/// process can only observe the previous complete generation or the next one.
pub struct AtomicCheckpointStore {
    path: PathBuf,
}

impl AtomicCheckpointStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self, checkpoint: &RecoveryCheckpoint) -> Result<(), CheckpointError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("checkpoint.json");
        let temporary = parent.join(format!(".{file_name}.{}.tmp", checkpoint.generation));
        let bytes = serde_json::to_vec_pretty(checkpoint)?;

        let write_result = (|| -> Result<(), std::io::Error> {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result.map_err(Into::into)
    }

    pub fn load(&self) -> Result<RecoveryCheckpoint, CheckpointError> {
        let checkpoint: RecoveryCheckpoint = serde_json::from_slice(&fs::read(&self.path)?)?;
        if checkpoint.schema_version != CHECKPOINT_VERSION {
            return Err(CheckpointError::UnsupportedVersion(
                checkpoint.schema_version,
            ));
        }
        Ok(checkpoint)
    }
}

/// Persist the full recovery state before acknowledging any source offset.
/// The callback owns its arguments so both real and test committers can use an
/// ordinary async closure.
pub async fn persist_then_commit<F, Fut>(
    store: &AtomicCheckpointStore,
    checkpoint: &RecoveryCheckpoint,
    consumer_groups: &BTreeMap<SignalSource, String>,
    mut commit: F,
) -> Result<BTreeMap<SignalSource, SourcePosition>, CheckpointError>
where
    F: FnMut(SignalSource, String, SourcePosition) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let plan = checkpoint
        .source_offsets
        .iter()
        .map(|(source, position)| {
            let group = consumer_groups
                .get(source)
                .cloned()
                .ok_or(CheckpointError::MissingConsumerGroup(*source))?;
            Ok((*source, group, position.clone()))
        })
        .collect::<Result<Vec<_>, CheckpointError>>()?;

    store.save(checkpoint)?;
    let mut committed = BTreeMap::new();
    for (source, group, position) in plan {
        commit(source, group, position.clone())
            .await
            .map_err(CheckpointError::Commit)?;
        committed.insert(source, position);
    }
    Ok(committed)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Duration;

    use super::*;
    use crate::windowing::EventTimeCorrelator;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "acteon-neural-checkpoint-{label}-{}.json",
            uuid::Uuid::new_v4()
        ))
    }

    fn checkpoint(generation: u64) -> RecoveryCheckpoint {
        let correlator =
            EventTimeCorrelator::new(Duration::minutes(1), Duration::seconds(15)).unwrap();
        RecoveryCheckpoint::new(
            generation,
            correlator.snapshot(),
            Vec::new(),
            BTreeMap::from([(
                SignalSource::Metrics,
                SourcePosition {
                    topic: "metrics".to_owned(),
                    partition: 0,
                    offset: 7,
                },
            )]),
        )
    }

    #[test]
    fn atomic_store_round_trips_a_generation() {
        let path = test_path("round-trip");
        let store = AtomicCheckpointStore::new(&path);
        store.save(&checkpoint(3)).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.generation, 3);
        assert_eq!(loaded.source_offsets[&SignalSource::Metrics].offset, 7);
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn checkpoint_is_visible_before_the_first_offset_commit() {
        let path = test_path("ordering");
        let store = AtomicCheckpointStore::new(&path);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let callback_observed = Arc::clone(&observed);
        let callback_path = path.clone();
        let groups = BTreeMap::from([(SignalSource::Metrics, "metrics-group".to_owned())]);

        persist_then_commit(
            &store,
            &checkpoint(4),
            &groups,
            move |source, group, position| {
                let observed = Arc::clone(&callback_observed);
                let path = callback_path.clone();
                async move {
                    let persisted: RecoveryCheckpoint =
                        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                            .map_err(|error| error.to_string())?;
                    observed.lock().unwrap().push((
                        persisted.generation,
                        source,
                        group,
                        position.offset,
                    ));
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(
            *observed.lock().unwrap(),
            vec![(4, SignalSource::Metrics, "metrics-group".to_owned(), 7)]
        );
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn failed_checkpoint_prevents_offset_commit() {
        let directory = test_path("directory");
        fs::create_dir(&directory).unwrap();
        let store = AtomicCheckpointStore::new(&directory);
        let calls = Arc::new(Mutex::new(0_u8));
        let callback_calls = Arc::clone(&calls);
        let groups = BTreeMap::from([(SignalSource::Metrics, "metrics-group".to_owned())]);

        let result = persist_then_commit(&store, &checkpoint(5), &groups, move |_, _, _| {
            let calls = Arc::clone(&callback_calls);
            async move {
                *calls.lock().unwrap() += 1;
                Ok(())
            }
        })
        .await;

        assert!(matches!(result, Err(CheckpointError::Io(_))));
        assert_eq!(*calls.lock().unwrap(), 0);
        fs::remove_dir(directory).unwrap();
    }
}
