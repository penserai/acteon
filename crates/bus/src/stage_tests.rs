use super::*;
use crate::{
    StreamCheckpointConfig, StreamPositionLane, StreamStageSourceError, stream_checkpoint_key,
};
use acteon_state::{StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn key() -> StateKey {
    stream_checkpoint_key("test", "tenant", "typed-stage")
}
fn record(offset: i64) -> StreamStageRecord<i64> {
    let mut message = BusMessage::new("test.tenant.input", json!(offset + 1));
    message.partition = Some(0);
    message.offset = Some(offset);
    StreamStageRecord {
        position: StreamPosition {
            lane: StreamPositionLane {
                source: "input".into(),
                topic: message.topic.clone(),
                consumer_group: "test-group".into(),
                partition: 0,
            },
            offset,
        },
        message,
        receipt: offset,
    }
}
struct Source {
    records: Vec<StreamStageRecord<i64>>,
    store: Arc<dyn StateStore>,
    fail_ack: bool,
    cancel_on_validate: Option<CancellationToken>,
    forge: bool,
    ack_calls: usize,
    receive_calls: usize,
}
impl Source {
    fn new(store: Arc<dyn StateStore>, offsets: std::ops::Range<i64>) -> Self {
        Self {
            records: offsets.map(record).collect(),
            store,
            fail_ack: false,
            cancel_on_validate: None,
            forge: false,
            ack_calls: 0,
            receive_calls: 0,
        }
    }
}
#[async_trait]
impl StreamStageSource for Source {
    type Receipt = i64;
    fn identity(&self) -> String {
        "test-source-v1".into()
    }
    async fn receive(
        &mut self,
        max: usize,
    ) -> Result<Vec<StreamStageRecord<i64>>, StreamStageSourceError> {
        self.receive_calls += 1;
        Ok(self.records.iter().take(max).cloned().collect())
    }
    async fn validate(
        &mut self,
        records: &[StreamStageRecord<i64>],
    ) -> Result<Vec<StreamPosition>, StreamStageSourceError> {
        assert!(
            records
                .iter()
                .zip(&self.records)
                .all(|(a, b)| a.receipt == b.receipt),
            "complete prefix"
        );
        if let Some(c) = &self.cancel_on_validate {
            c.cancel();
        }
        let mut positions = records
            .iter()
            .map(|r| r.position.clone())
            .collect::<Vec<_>>();
        if self.forge {
            positions[0].offset += 1000;
        }
        Ok(positions)
    }
    async fn acknowledge(
        &mut self,
        records: &[StreamStageRecord<i64>],
    ) -> Result<(), StreamStageSourceError> {
        self.ack_calls += 1;
        let checkpoint: StreamCheckpointSnapshot<u64, u64> =
            serde_json::from_str(&self.store.get(&key()).await.unwrap().unwrap()).unwrap();
        assert!(
            records.iter().all(|r| checkpoint
                .positions()
                .iter()
                .any(|p| p.lane == r.position.lane && p.offset >= r.position.offset)),
            "input progress must already be durable"
        );
        if std::mem::take(&mut self.fail_ack) {
            return Err(StreamStageSourceError::Fenced(
                "lost acknowledgement after checkpoint".into(),
            ));
        }
        self.records.drain(..records.len());
        Ok(())
    }
    async fn recover(&mut self) -> Result<(), StreamStageSourceError> {
        Ok(())
    }
    async fn close(&mut self) {}
}
struct Processor {
    calls: Arc<AtomicUsize>,
    failures: AtomicUsize,
    permanent: bool,
}
impl Processor {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            failures: AtomicUsize::new(0),
            permanent: false,
        }
    }
}
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for Processor {
    #[allow(deprecated)] // Keep the atomic retry primitive compatible with Rust 1.88.
    async fn process(
        &self,
        state: u64,
        inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(if self.permanent {
                StreamStageProcessError::Permanent("poison".into())
            } else {
                StreamStageProcessError::Retryable("temporary inference error".into())
            });
        }
        Ok(StreamStageTransition {
            state: state + inputs.iter().map(|i| i.payload).sum::<u64>(),
            outputs: inputs
                .iter()
                .map(|i| StreamOutboxEntry {
                    idempotency_key: format!("input:{}", i.position.offset),
                    created_at: Utc::now(),
                    payload: i.payload,
                })
                .collect(),
        })
    }
}
async fn coordinator(store: Arc<dyn StateStore>) -> StreamCheckpointCoordinator<u64, u64> {
    StreamCheckpointCoordinator::initialize(store, key(), 0, StreamCheckpointConfig::default())
        .await
        .unwrap()
}
async fn stage(
    store: Arc<dyn StateStore>,
    config: StreamStageConfig,
) -> ManagedStreamStage<u64, u64> {
    ManagedStreamStage::initialize(
        coordinator(store).await,
        "worker",
        "processor-v1",
        "test-source-v1",
        config,
    )
    .await
    .unwrap()
}
async fn expire(store: Arc<dyn StateStore>) {
    let mut c = coordinator(store).await;
    let mut next = c.snapshot.clone();
    next.processing
        .as_mut()
        .unwrap()
        .lease
        .as_mut()
        .unwrap()
        .expires_at = Utc::now() - TimeDelta::seconds(1);
    c.persist(next).await.unwrap();
}

#[tokio::test]
async fn completed_checkpoint_recovery_does_not_repeat_inference_or_outputs() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let p = Processor::new();
    let mut s = Source::new(store.clone(), 0..3);
    s.fail_ack = true;
    let mut original = stage(store.clone(), StreamStageConfig::default()).await;
    assert!(matches!(
        original
            .process_once(&mut s, &p, &CancellationToken::new())
            .await,
        Err(StreamStageError::Acknowledgement { generation: 1, .. })
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    drop(original);
    let mut replacement = stage(store.clone(), StreamStageConfig::default()).await;
    assert_eq!(
        replacement
            .process_once(&mut s, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Completed {
            generation: 1,
            processed: 0,
            recovered: 3
        }
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*replacement.checkpoint().state(), 6);
    assert_eq!(replacement.checkpoint().pending_outputs().len(), 3);
    assert_eq!(s.ack_calls, 2);
    assert_eq!(
        replacement
            .metrics()
            .await
            .unwrap()
            .counters
            .acknowledgement_failures,
        1
    );
}

#[tokio::test]
async fn completed_work_can_be_checkpointed_on_late_cancellation_without_acknowledgement() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let p = Processor::new();
    let mut source = Source::new(store.clone(), 0..2);
    let cancel = CancellationToken::new();
    source.cancel_on_validate = Some(cancel.clone());
    let mut old = stage(store.clone(), StreamStageConfig::default()).await;
    assert_eq!(
        old.process_once(&mut source, &p, &cancel).await.unwrap(),
        StreamStageResult::Cancelled {
            durable_generation: Some(1)
        }
    );
    assert_eq!(source.ack_calls, 0);
    source.cancel_on_validate = None;
    let mut new = stage(store, StreamStageConfig::default()).await;
    new.process_once(&mut source, &p, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(source.ack_calls, 1);
}

#[tokio::test]
async fn retries_and_attempt_budget_survive_worker_replacement_and_do_not_skip_poison() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let p = Processor {
        failures: AtomicUsize::new(10),
        ..Processor::new()
    };
    let mut source = Source::new(store.clone(), 0..1);
    let config = StreamStageConfig {
        max_attempts: 2,
        initial_backoff_ms: 1,
        max_backoff_ms: 1,
        ..Default::default()
    };
    let mut old = stage(store.clone(), config.clone()).await;
    assert!(matches!(
        old.process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::RetryScheduled { attempts: 1, .. }
    ));
    drop(old);
    tokio::time::sleep(Duration::from_millis(5)).await;
    let mut replacement = stage(store, config).await;
    assert!(matches!(
        replacement
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { attempts: 2, .. }
    ));
    assert_eq!(source.ack_calls, 0);
    assert_eq!(*replacement.checkpoint().state(), 0);
    assert!(replacement.checkpoint().positions().is_empty());
    assert_eq!(p.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        replacement
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { attempts: 2, .. }
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn backpressure_precedes_receiving_or_invoking_the_processor() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let config = StreamStageConfig {
        max_outputs_per_batch: 1,
        outbox_high_watermark: 1,
        max_batch_records: 1,
        ..Default::default()
    };
    let mut worker = stage(store.clone(), config).await;
    let p = Processor::new();
    let mut source = Source::new(store, 0..2);
    worker
        .process_once(&mut source, &p, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        worker
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Backpressured
    );
    assert_eq!(source.receive_calls, 1);
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    worker
        .coordinator
        .acknowledge_outputs(["input:0"])
        .await
        .unwrap();
    worker
        .process_once(&mut source, &p, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn changed_definitions_raw_checkpoint_writes_and_forged_offsets_are_rejected() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    assert!(matches!(
        ManagedStreamStage::initialize(
            coordinator(store.clone()).await,
            "worker",
            "processor-v2",
            "test-source-v1",
            StreamStageConfig::default()
        )
        .await,
        Err(StreamStageError::DefinitionMismatch)
    ));
    assert!(matches!(
        worker.coordinator.checkpoint(999, [], []).await,
        Err(StreamCheckpointError::ManagedProcessing)
    ));
    let mut source = Source::new(store, 0..1);
    source.forge = true;
    assert!(matches!(
        worker
            .process_once(&mut source, &Processor::new(), &CancellationToken::new())
            .await,
        Err(StreamStageError::InvalidBatch(_))
    ));
    assert_eq!(source.ack_calls, 0);
    assert_eq!(*worker.checkpoint().state(), 0);
}

struct Blocking {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for Blocking {
    async fn process(
        &self,
        state: u64,
        inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Processor::new().process(state, inputs).await
    }
}
#[tokio::test]
async fn expired_worker_cannot_publish_over_a_replacement_checkpoint() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let old = stage(store.clone(), StreamStageConfig::default()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let blocker = Blocking {
        entered: entered.clone(),
        release: release.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let source_store = store.clone();
    let task = tokio::spawn(async move {
        let mut worker = old;
        let mut source = Source::new(source_store, 0..1);
        let result = worker
            .process_once(&mut source, &blocker, &CancellationToken::new())
            .await;
        (result, source.ack_calls)
    });
    entered.notified().await;
    let mut peer = stage(store.clone(), StreamStageConfig::default()).await;
    let mut source = Source::new(store.clone(), 0..1);
    assert_eq!(
        peer.process_once(&mut source, &Processor::new(), &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Busy
    );
    expire(store.clone()).await;
    peer.process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    release.notify_one();
    let (result, acks) = task.await.unwrap();
    assert!(matches!(result, Err(StreamStageError::Fenced)));
    assert_eq!(acks, 0);
    assert_eq!(peer.checkpoint().pending_outputs().len(), 1);
    assert_eq!(peer.metrics().await.unwrap().counters.recovered_leases, 1);
}
#[tokio::test]
async fn cancellation_during_processing_keeps_state_and_outputs_unadvanced() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let old = stage(store.clone(), StreamStageConfig::default()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task_store = store.clone();
    let blocker = Blocking {
        entered: entered.clone(),
        release: Arc::new(tokio::sync::Notify::new()),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let task = tokio::spawn(async move {
        let mut worker = old;
        let mut source = Source::new(task_store, 0..1);
        let result = worker
            .process_once(&mut source, &blocker, &task_cancel)
            .await;
        (result, source.ack_calls)
    });
    entered.notified().await;
    cancel.cancel();
    let (r, acks) = task.await.unwrap();
    assert_eq!(
        r.unwrap(),
        StreamStageResult::Cancelled {
            durable_generation: None
        }
    );
    assert_eq!(acks, 0);
    let mut replacement = stage(store.clone(), StreamStageConfig::default()).await;
    assert_eq!(*replacement.checkpoint().state(), 0);
    replacement
        .process_once(
            &mut Source::new(store, 0..1),
            &Processor::new(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(replacement.metrics().await.unwrap().counters.attempts, 2);
}

#[tokio::test]
async fn malformed_typed_input_halts_without_acknowledgement() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    let mut source = Source::new(store, 0..1);
    source.records[0].message.payload = json!({"wrong":"shape"});
    let p = Processor::new();
    assert!(matches!(
        worker
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { attempts: 1, .. }
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.ack_calls, 0);
}

#[tokio::test]
async fn crash_on_last_attempt_halts_after_lease_expiry_without_another_callback() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let config = StreamStageConfig {
        max_attempts: 1,
        ..Default::default()
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let blocker = Blocking {
        entered: entered.clone(),
        release: Arc::new(tokio::sync::Notify::new()),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let mut old = stage(store.clone(), config.clone()).await;
    let task_store = store.clone();
    let task = tokio::spawn(async move {
        old.process_once(
            &mut Source::new(task_store, 0..1),
            &blocker,
            &CancellationToken::new(),
        )
        .await
    });
    entered.notified().await;
    task.abort();
    let _ = task.await;
    expire(store.clone()).await;
    let mut replacement = stage(store.clone(), config).await;
    let p = Processor::new();
    let mut source = Source::new(store, 0..1);
    assert!(matches!(
        replacement
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { attempts: 1, .. }
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.receive_calls, 0);
    assert_eq!(source.ack_calls, 0);
}

#[tokio::test]
async fn input_and_output_bounds_leave_progress_uncommitted() {
    for input_bound in [true, false] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let config = if input_bound {
            StreamStageConfig {
                max_batch_bytes: 1,
                ..Default::default()
            }
        } else {
            StreamStageConfig {
                max_output_bytes: 1,
                ..Default::default()
            }
        };
        let mut worker = stage(store.clone(), config).await;
        let mut source = Source::new(store, 0..1);
        let p = Processor::new();
        let result = worker
            .process_once(&mut source, &p, &CancellationToken::new())
            .await;
        if input_bound {
            assert!(matches!(result, Err(StreamStageError::InvalidBatch(_))));
        } else {
            assert!(matches!(result.unwrap(), StreamStageResult::Halted { .. }));
        }
        assert_eq!(worker.checkpoint().positions(), []);
        assert_eq!(worker.checkpoint().pending_outputs(), []);
        assert_eq!(source.ack_calls, 0);
    }
}

struct LostWriteReply {
    inner: MemoryStateStore,
    lost: std::sync::atomic::AtomicBool,
    replay_completion: bool,
    control_completion: bool,
}
#[async_trait]
impl StateStore for LostWriteReply {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, acteon_state::StateError> {
        self.inner.check_and_set(key, value, ttl).await
    }
    async fn get(&self, key: &StateKey) -> Result<Option<String>, acteon_state::StateError> {
        self.inner.get(key).await
    }
    async fn get_versioned(
        &self,
        key: &StateKey,
    ) -> Result<Option<(String, u64)>, acteon_state::StateError> {
        self.inner.get_versioned(key).await
    }
    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), acteon_state::StateError> {
        self.inner.set(key, value, ttl).await
    }
    async fn delete(&self, key: &StateKey) -> Result<bool, acteon_state::StateError> {
        self.inner.delete(key).await
    }
    async fn compare_and_delete(
        &self,
        key: &StateKey,
        expected_version: u64,
    ) -> Result<bool, acteon_state::StateError> {
        self.inner.compare_and_delete(key, expected_version).await
    }
    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, acteon_state::StateError> {
        self.inner.increment(key, delta, ttl).await
    }
    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<acteon_state::CasResult, acteon_state::StateError> {
        let result = self
            .inner
            .compare_and_swap(key, expected_version, new_value, ttl)
            .await?;
        let saved: serde_json::Value = serde_json::from_str(new_value).unwrap();
        if result == acteon_state::CasResult::Ok
            && if self.control_completion {
                saved["processing"]["controls"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
            } else if self.replay_completion {
                saved["processing"]["replays"]
                    .as_array()
                    .is_some_and(|jobs| jobs.iter().any(|j| j["audit"]["status"] == "completed"))
            } else {
                saved["generation"] == 1
            }
            && !self.lost.swap(true, Ordering::SeqCst)
        {
            return Err(acteon_state::StateError::Connection(
                "lost response after durable write".into(),
            ));
        }
        Ok(result)
    }
    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: acteon_state::KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.inner.scan_keys(namespace, tenant, kind, prefix).await
    }
    async fn scan_keys_by_kind(
        &self,
        kind: acteon_state::KeyKind,
    ) -> Result<Vec<(String, String)>, acteon_state::StateError> {
        self.inner.scan_keys_by_kind(kind).await
    }
    async fn index_timeout(
        &self,
        key: &StateKey,
        expires_at_ms: i64,
    ) -> Result<(), acteon_state::StateError> {
        self.inner.index_timeout(key, expires_at_ms).await
    }
    async fn remove_timeout_index(&self, key: &StateKey) -> Result<(), acteon_state::StateError> {
        self.inner.remove_timeout_index(key).await
    }
    async fn get_expired_timeouts(
        &self,
        now_ms: i64,
    ) -> Result<Vec<String>, acteon_state::StateError> {
        self.inner.get_expired_timeouts(now_ms).await
    }
}

#[tokio::test]
async fn ambiguous_checkpoint_write_is_reloaded_before_recovery_processing() {
    let store: Arc<dyn StateStore> = Arc::new(LostWriteReply {
        inner: MemoryStateStore::new(),
        lost: std::sync::atomic::AtomicBool::new(false),
        replay_completion: false,
        control_completion: false,
    });
    let p = Processor::new();
    let mut source = Source::new(store.clone(), 0..1);
    let mut original = stage(store.clone(), StreamStageConfig::default()).await;
    assert!(matches!(
        original
            .process_once(&mut source, &p, &CancellationToken::new())
            .await,
        Err(StreamStageError::Checkpoint(StreamCheckpointError::State(
            _
        )))
    ));
    assert_eq!(source.ack_calls, 0);
    expire(store.clone()).await;
    let mut replacement = stage(store, StreamStageConfig::default()).await;
    assert!(matches!(
        replacement
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Completed {
            processed: 0,
            recovered: 1,
            ..
        }
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.checkpoint().pending_outputs().len(), 1);
}

#[tokio::test]
async fn concurrent_output_acknowledgement_is_preserved_when_processing_finishes() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    let mut source = Source::new(store.clone(), 0..1);
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let processor = Blocking {
        entered: entered.clone(),
        release: release.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let task_store = store.clone();
    let task = tokio::spawn(async move {
        worker
            .process_once(
                &mut Source::new(task_store, 1..2),
                &processor,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        worker
    });
    entered.notified().await;
    coordinator(store)
        .await
        .acknowledge_outputs(["input:0"])
        .await
        .unwrap();
    release.notify_one();
    let worker = task.await.unwrap();
    assert_eq!(*worker.checkpoint().state(), 3);
    let pending = worker.checkpoint().pending_outputs();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].idempotency_key, "input:1");
}

#[test]
fn oversized_timestamp_policies_are_rejected_without_panicking() {
    let config = StreamStageConfig {
        lease_ms: i64::MAX as u64,
        ..Default::default()
    };
    assert!(config.validate().is_err());
    assert!(bounded_error("🦀".repeat(2048)).len() <= 4096);
}

fn consume_config() -> StreamStageConfig {
    let contract = crate::StreamInputContract::from_schema(&acteon_core::Schema::new(
        "input",
        1,
        "test",
        "tenant",
        json!({"type":"integer","minimum":1}),
    ))
    .unwrap();
    StreamStageConfig {
        input: StreamInputPolicy {
            contracts: BTreeMap::from([("input".into(), contract)]),
            poison_policy: StreamPoisonPolicy::Quarantine,
            ..Default::default()
        },
        ..Default::default()
    }
}
#[tokio::test]
async fn consume_contract_quarantines_bad_input_before_ack_and_recovery_does_not_repeat_it() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut source = Source::new(store.clone(), 0..3);
    source.records[1].message.payload = json!(-1);
    source.fail_ack = true;
    let processor = Processor::new();
    let config = consume_config();
    let mut worker = stage(store.clone(), config.clone()).await;
    assert!(matches!(
        worker
            .process_once(&mut source, &processor, &CancellationToken::new())
            .await,
        Err(StreamStageError::Acknowledgement { generation: 1, .. })
    ));
    assert_eq!(*worker.checkpoint().state(), 4);
    assert_eq!(worker.checkpoint().pending_outputs().len(), 2);
    let quarantined = worker.quarantined_inputs().await.unwrap();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].position.offset, 1);
    assert_eq!(quarantined[0].message.payload, json!(-1));
    assert_eq!(quarantined[0].failure, StreamInputFailure::SchemaViolation);
    assert_eq!(
        quarantined[0].contract_sha256,
        Some(config.input.contracts["input"].sha256.clone())
    );
    let mut replacement = stage(store, config).await;
    replacement
        .process_once(&mut source, &processor, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(processor.calls.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.quarantined_inputs().await.unwrap(), quarantined);
    assert_eq!(
        replacement
            .metrics()
            .await
            .unwrap()
            .counters
            .quarantined_records,
        1
    );
}
#[tokio::test]
async fn all_poison_batch_never_invokes_processor_and_discard_is_explicit_and_idempotent() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut source = Source::new(store, 0..1);
    source.records[0].message.payload = json!(0);
    let processor = Processor::new();
    worker
        .process_once(&mut source, &processor, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(processor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.ack_calls, 1);
    assert_eq!(*worker.checkpoint().state(), 0);
    let id = worker.quarantined_inputs().await.unwrap()[0].id.clone();
    assert!(!worker.discard_quarantined_input("wrong-id").await.unwrap());
    assert!(worker.discard_quarantined_input(&id).await.unwrap());
    assert!(!worker.discard_quarantined_input(&id).await.unwrap());
    assert_eq!(
        worker
            .metrics()
            .await
            .unwrap()
            .counters
            .discarded_quarantined_records,
        1
    );
}
#[tokio::test]
async fn quarantine_capacity_blocks_before_callback_and_does_not_spend_attempt_budget() {
    for byte_limit in [false, true] {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let mut config = consume_config();
        config.max_batch_records = 1;
        config.input.max_quarantined_records = 1;
        if byte_limit {
            config.input.max_quarantine_bytes = 1;
        }
        let mut worker = stage(store.clone(), config).await;
        let mut source = Source::new(store, 0..2);
        for r in &mut source.records {
            r.message.payload = json!(0);
        }
        let processor = Processor::new();
        if !byte_limit {
            worker
                .process_once(&mut source, &processor, &CancellationToken::new())
                .await
                .unwrap();
        }
        let before = worker.metrics().await.unwrap().counters.attempts;
        for _ in 0..3 {
            assert_eq!(
                worker
                    .process_once(&mut source, &processor, &CancellationToken::new())
                    .await
                    .unwrap(),
                StreamStageResult::Backpressured
            );
        }
        assert_eq!(worker.metrics().await.unwrap().counters.attempts, before);
        assert_eq!(processor.calls.load(Ordering::SeqCst), 0);
        assert_eq!(source.ack_calls, usize::from(!byte_limit));
        if !byte_limit {
            let id = worker.quarantined_inputs().await.unwrap()[0].id.clone();
            worker.discard_quarantined_input(&id).await.unwrap();
            worker
                .process_once(&mut source, &processor, &CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(source.ack_calls, 2);
        }
    }
}
#[tokio::test]
async fn quarantine_checkpoint_lost_reply_recovers_without_duplicate_retention() {
    let store: Arc<dyn StateStore> = Arc::new(LostWriteReply {
        inner: MemoryStateStore::new(),
        lost: std::sync::atomic::AtomicBool::new(false),
        replay_completion: false,
        control_completion: false,
    });
    let mut source = Source::new(store.clone(), 0..1);
    source.records[0].message.payload = json!(0);
    let mut worker = stage(store.clone(), consume_config()).await;
    let processor = Processor::new();
    assert!(
        worker
            .process_once(&mut source, &processor, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(source.ack_calls, 0);
    expire(store.clone()).await;
    worker = stage(store, consume_config()).await;
    worker
        .process_once(&mut source, &processor, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(worker.quarantined_inputs().await.unwrap().len(), 1);
    assert_eq!(processor.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn changed_consume_contracts_and_unbound_sources_fail_closed() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut changed = consume_config();
    changed.input.contracts.get_mut("input").unwrap().version = 2;
    assert!(matches!(
        ManagedStreamStage::initialize(
            coordinator(store.clone()).await,
            "worker",
            "processor-v1",
            "test-source-v1",
            changed
        )
        .await,
        Err(StreamStageError::DefinitionMismatch)
    ));
    let mut source = Source::new(store, 0..1);
    source.records[0].position.lane.source = "unbound".into();
    assert!(matches!(
        worker
            .process_once(&mut source, &Processor::new(), &CancellationToken::new())
            .await,
        Err(StreamStageError::InvalidConfig(_))
    ));
    assert_eq!(source.ack_calls, 0);
}
#[tokio::test]
async fn typed_decode_can_quarantine_even_when_json_schema_accepts_the_payload() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut config = consume_config();
    config.input.contracts.clear();
    let mut worker = stage(store.clone(), config).await;
    let mut source = Source::new(store, 0..1);
    source.records[0].message.payload = json!("private-payload");
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let retained = worker.quarantined_inputs().await.unwrap();
    assert_eq!(retained[0].failure, StreamInputFailure::TypedDecode);
    assert!(!retained[0].reason.contains("private-payload"));
}
#[test]
fn consume_contracts_reject_external_refs_bad_digests_and_invalid_schemas() {
    let schema = |body| acteon_core::Schema::new("test", 1, "test", "tenant", body);
    for body in [
        json!({"$ref":"file:///etc/passwd"}),
        json!({"$ref":"http://127.0.0.1/private"}),
        json!({"type":42}),
    ] {
        assert!(crate::StreamInputContract::from_schema(&schema(body)).is_err());
    }
    let mut pinned =
        crate::StreamInputContract::from_schema(&schema(json!({"type":"integer"}))).unwrap();
    pinned.body = json!({"type":"string"});
    assert!(pinned.compile().is_err());
    assert!(
        crate::StreamInputContract::from_schema(&schema(
            json!({"$defs":{"integer":{"type":"integer"}},"$ref":"#/$defs/integer"})
        ))
        .is_ok()
    );
}

#[tokio::test]
async fn corrupted_quarantine_snapshots_are_rejected_before_recovery() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut source = Source::new(store.clone(), 0..1);
    source.records[0].message.payload = json!(0);
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let saved: serde_json::Value =
        serde_json::from_str(&store.get(&key()).await.unwrap().unwrap()).unwrap();
    assert_eq!(saved["schema_version"], 4);
    for mutation in 0..5 {
        let mut corrupt = saved.clone();
        match mutation {
            0 => corrupt["positions"] = json!([]),
            1 => corrupt["processing"]["quarantine"][0]["message"]["offset"] = json!(99),
            2 => corrupt["processing"]["quarantine"][0]["contract_sha256"] = json!("forged"),
            3 => corrupt["processing"]["counters"]["quarantined_records"] = json!(2),
            _ => corrupt["schema_version"] = json!(3),
        }
        store.set(&key(), &corrupt.to_string(), None).await.unwrap();
        assert!(matches!(
            StreamCheckpointCoordinator::<u64, u64>::initialize(
                store.clone(),
                key(),
                0,
                StreamCheckpointConfig::default()
            )
            .await,
            Err(StreamCheckpointError::InvalidManagedStage(_))
        ));
    }
}
#[tokio::test]
async fn schema_rejection_halts_by_default_without_advancing_offsets() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut config = consume_config();
    config.input.poison_policy = StreamPoisonPolicy::Halt;
    let mut worker = stage(store.clone(), config).await;
    let mut source = Source::new(store, 0..1);
    source.records[0].message.payload = json!(0);
    let processor = Processor::new();
    assert!(matches!(
        worker
            .process_once(&mut source, &processor, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { .. }
    ));
    assert_eq!(processor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(source.ack_calls, 0);
    assert_eq!(worker.checkpoint().positions(), []);
}

#[tokio::test]
async fn forged_receipt_positions_cannot_persist_or_acknowledge_quarantine() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut source = Source::new(store, 0..1);
    source.records[0].message.payload = json!(0);
    source.forge = true;
    assert!(matches!(
        worker
            .process_once(&mut source, &Processor::new(), &CancellationToken::new())
            .await,
        Err(StreamStageError::InvalidBatch(_))
    ));
    assert_eq!(source.ack_calls, 0);
    assert_eq!(worker.checkpoint().positions(), []);
    assert_eq!(worker.quarantined_inputs().await.unwrap(), []);
}

#[tokio::test]
async fn operator_access_is_read_only_and_discard_preserves_newer_worker_progress() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    assert!(
        StreamStageOperator::load(store.clone(), key())
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get(&key()).await.unwrap().is_none());
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut source = Source::new(store.clone(), 0..2);
    source.records[0].message.payload = json!(0);
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let before = store.get_versioned(&key()).await.unwrap();
    let mut operator = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let id = operator.quarantined_inputs()[0].id.clone();
    assert_eq!(operator.status().quarantined_records, 1);
    assert_eq!(before, store.get_versioned(&key()).await.unwrap());
    let mut next_source = Source::new(store.clone(), 2..3);
    worker
        .process_once(
            &mut next_source,
            &Processor::new(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(operator.discard(&id).await.unwrap());
    assert!(!operator.discard(&id).await.unwrap());
    let c = coordinator(store.clone()).await;
    assert_eq!(*c.snapshot().state(), 5);
    assert_eq!(c.snapshot().pending_outputs().len(), 2);
    assert_eq!(operator.status().counters.processed_records, 3);
    assert_eq!(operator.status().counters.discarded_quarantined_records, 1);
    assert_eq!(worker.metrics().await.unwrap().quarantined_records, 0);
    assert!(
        StreamStageOperator::load(store, stream_checkpoint_key("test", "other", "typed-stage"))
            .await
            .unwrap()
            .is_none()
    );
}

async fn replay_fixture(
    config: StreamStageConfig,
) -> (Arc<dyn StateStore>, ManagedStreamStage<u64, u64>, String) {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), config).await;
    let mut source = Source::new(store.clone(), 0..1);
    source.records[0].message.payload = json!(0);
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let id = worker.quarantined_inputs().await.unwrap()[0].id.clone();
    (store, worker, id)
}
#[tokio::test]
async fn replay_is_audited_idempotent_and_atomically_commits_without_source_progress() {
    let (store, mut worker, id) = replay_fixture(consume_config()).await;
    let positions = worker.checkpoint().positions().to_vec();
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let request = Uuid::new_v4();
    assert!(
        op.request_replay(request, &id, "operator", "repair", json!(0))
            .await
            .is_err()
    );
    let audit = op
        .request_replay(request, &id, "operator", "repair", json!(7))
        .await
        .unwrap();
    assert_eq!(audit.status, StreamReplayStatus::Pending);
    assert_eq!(
        audit,
        op.request_replay(request, &id, "operator", "repair", json!(7))
            .await
            .unwrap()
    );
    assert!(matches!(
        op.request_replay(request, &id, "other", "repair", json!(7))
            .await,
        Err(StreamStageError::ReplayConflict)
    ));
    assert!(matches!(
        op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(8))
            .await,
        Err(StreamStageError::ReplayConflict)
    ));
    assert!(matches!(
        op.discard(&id).await,
        Err(StreamStageError::ReplayConflict)
    ));
    assert!(matches!(
        worker.discard_quarantined_input(&id).await,
        Err(StreamStageError::ReplayConflict)
    ));
    let processor = Processor::new();
    assert!(
        matches!(worker.replay_once(&processor,&CancellationToken::new()).await.unwrap(),StreamStageResult::ReplayCompleted{request_id,..} if request_id==request)
    );
    assert_eq!(worker.checkpoint().positions(), positions);
    assert_eq!(*worker.checkpoint().state(), 7);
    assert_eq!(worker.checkpoint().pending_outputs().len(), 1);
    assert_eq!(
        worker
            .metrics()
            .await
            .unwrap()
            .counters
            .replayed_quarantined_records,
        1
    );
    assert!(worker.quarantined_inputs().await.unwrap().is_empty());
    let completed = op
        .request_replay(request, &id, "operator", "repair", json!(7))
        .await
        .unwrap();
    assert_eq!(completed.status, StreamReplayStatus::Completed);
    assert_eq!(completed.attempts, 1);
    assert_eq!(
        completed.completed_generation,
        Some(worker.checkpoint().generation())
    );
    assert_ne!(
        completed.original_payload_sha256,
        completed.repaired_payload_sha256
    );
    let mut replacement = stage(store.clone(), consume_config()).await;
    assert_eq!(
        replacement
            .replay_once(&processor, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Idle
    );
    assert_eq!(processor.calls.load(Ordering::SeqCst), 1);
    let raw: serde_json::Value =
        serde_json::from_str(&store.get(&key()).await.unwrap().unwrap()).unwrap();
    assert_eq!(raw["schema_version"], 5);
}
#[tokio::test]
async fn replay_retry_budget_survives_interrupted_worker_and_preserves_original() {
    let mut config = consume_config();
    config.max_attempts = 2;
    config.initial_backoff_ms = 1;
    let (store, mut worker, id) = replay_fixture(config.clone()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let request = Uuid::new_v4();
    op.request_replay(request, &id, "operator", "repair", json!(5))
        .await
        .unwrap();
    let p = Processor::new();
    p.failures.store(1, Ordering::SeqCst);
    assert!(matches!(
        worker
            .replay_once(&p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayRetryScheduled { .. }
    ));
    assert_eq!(*worker.checkpoint().state(), 0);
    worker.reload().await.unwrap();
    let mut next = worker.coordinator.snapshot.clone();
    let a = &mut next.processing.as_mut().unwrap().replays[0].audit;
    a.status = StreamReplayStatus::Running;
    a.attempts = 2;
    a.next_attempt_at = None;
    worker.persist(next).await.unwrap();
    let mut replacement = stage(store, config).await;
    assert!(
        matches!(replacement.replay_once(&p,&CancellationToken::new()).await.unwrap(),StreamStageResult::ReplayFailed{request_id} if request_id==request)
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.quarantined_inputs().await.unwrap().len(), 1);
    assert!(replacement.discard_quarantined_input(&id).await.unwrap());
}
#[tokio::test]
async fn replay_audit_capacity_reserves_room_for_error_outcomes() {
    let mut config = consume_config();
    config.max_replay_bytes = 100;
    let (store, mut worker, id) = replay_fixture(config).await;
    let mut op = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(1))
            .await,
        Err(StreamStageError::ReplayCapacity)
    ));
    assert_eq!(worker.quarantined_inputs().await.unwrap().len(), 1);
    assert!(op.replay_audits().is_empty());
}

#[tokio::test]
async fn replay_drains_through_normal_worker_loop_without_receiving_or_acknowledging() {
    let (store, mut worker, id) = replay_fixture(consume_config()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(3))
        .await
        .unwrap();
    let mut source = Source::new(store, 1..2);
    assert!(matches!(
        worker
            .process_once(&mut source, &Processor::new(), &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayCompleted { .. }
    ));
    assert_eq!(source.receive_calls, 0);
    assert_eq!(source.ack_calls, 0);
    assert_eq!(worker.checkpoint().positions()[0].offset, 0);
}
struct BlockReplay {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for BlockReplay {
    async fn process(
        &self,
        state: u64,
        _inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(StreamStageTransition {
            state: state + 999,
            outputs: vec![StreamOutboxEntry {
                idempotency_key: "stale-worker-output".into(),
                created_at: Utc::now(),
                payload: 999,
            }],
        })
    }
}
#[tokio::test]
async fn expired_replay_worker_cannot_overwrite_replacement_outcome() {
    let (store, mut worker, id) = replay_fixture(consume_config()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(7))
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let processor = BlockReplay {
        entered: entered.clone(),
        release: release.clone(),
    };
    let old = tokio::spawn(async move {
        worker
            .replay_once(&processor, &CancellationToken::new())
            .await
    });
    entered.notified().await;
    expire(store.clone()).await;
    let mut replacement = stage(store, consume_config()).await;
    assert!(matches!(
        replacement
            .replay_once(&Processor::new(), &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayCompleted { .. }
    ));
    release.notify_one();
    assert!(matches!(old.await.unwrap(), Err(StreamStageError::Fenced)));
    assert_eq!(*replacement.checkpoint().state(), 7);
    assert_eq!(replacement.checkpoint().pending_outputs().len(), 1);
    assert_eq!(
        replacement.checkpoint().pending_outputs()[0].idempotency_key,
        "input:0"
    );
}
struct EscapedReplayFailure;
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for EscapedReplayFailure {
    async fn process(
        &self,
        _state: u64,
        _inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        Err(StreamStageProcessError::Permanent("\0".repeat(4096)))
    }
}
#[tokio::test]
async fn admitted_replay_can_persist_worst_case_escaped_error_within_audit_budget() {
    let mut config = consume_config();
    config.max_replay_bytes = 32 * 1024;
    let (store, mut worker, id) = replay_fixture(config).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(2))
        .await
        .unwrap();
    assert!(matches!(
        worker
            .replay_once(&EscapedReplayFailure, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayFailed { .. }
    ));
    let reloaded = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.replay_audits()[0].status,
        StreamReplayStatus::Failed
    );
    assert!(
        reloaded.replay_audits()[0]
            .last_error
            .as_ref()
            .unwrap()
            .contains('\0')
    );
    assert_eq!(reloaded.quarantined_inputs().len(), 1);
}

#[tokio::test]
async fn replay_checkpoint_lost_response_does_not_repeat_callback_or_outputs() {
    let store: Arc<dyn StateStore> = Arc::new(LostWriteReply {
        inner: MemoryStateStore::new(),
        lost: std::sync::atomic::AtomicBool::new(false),
        replay_completion: true,
        control_completion: false,
    });
    let mut worker = stage(store.clone(), consume_config()).await;
    let mut source = Source::new(store.clone(), 0..1);
    source.records[0].message.payload = json!(0);
    worker
        .process_once(&mut source, &Processor::new(), &CancellationToken::new())
        .await
        .unwrap();
    let id = worker.quarantined_inputs().await.unwrap()[0].id.clone();
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let request = Uuid::new_v4();
    op.request_replay(request, &id, "operator", "repair", json!(8))
        .await
        .unwrap();
    let processor = Processor::new();
    assert!(
        worker
            .replay_once(&processor, &CancellationToken::new())
            .await
            .is_err()
    );
    let mut replacement = stage(store, consume_config()).await;
    assert_eq!(
        replacement
            .replay_once(&processor, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Idle
    );
    assert_eq!(*replacement.checkpoint().state(), 8);
    assert_eq!(replacement.checkpoint().pending_outputs().len(), 1);
    assert!(replacement.quarantined_inputs().await.unwrap().is_empty());
    assert_eq!(processor.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        op.request_replay(request, &id, "operator", "repair", json!(8))
            .await
            .unwrap()
            .status,
        StreamReplayStatus::Completed
    );
}
struct InvalidReplayTransition;
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for InvalidReplayTransition {
    async fn process(
        &self,
        state: u64,
        _inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        Ok(StreamStageTransition {
            state,
            outputs: vec![StreamOutboxEntry {
                idempotency_key: String::new(),
                created_at: Utc::now(),
                payload: 1,
            }],
        })
    }
}
#[tokio::test]
async fn invalid_replay_output_is_audited_failed_instead_of_abandoning_the_worker() {
    let (store, mut worker, id) = replay_fixture(consume_config()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(2))
        .await
        .unwrap();
    assert!(matches!(
        worker
            .replay_once(&InvalidReplayTransition, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayFailed { .. }
    ));
    assert!(worker.checkpoint().pending_outputs().is_empty());
    assert_eq!(worker.quarantined_inputs().await.unwrap().len(), 1);
    let op = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(op.replay_audits()[0].status, StreamReplayStatus::Failed);
}
#[tokio::test]
async fn typed_replay_decode_failure_never_invokes_the_processor() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut config = consume_config();
    config.input.contracts.clear();
    let mut worker = stage(store.clone(), config).await;
    let mut source = Source::new(store.clone(), 0..1);
    source.records[0].message.payload = json!("bad integer");
    let processor = Processor::new();
    worker
        .process_once(&mut source, &processor, &CancellationToken::new())
        .await
        .unwrap();
    let id = worker.quarantined_inputs().await.unwrap()[0].id.clone();
    let mut op = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(
        Uuid::new_v4(),
        &id,
        "operator",
        "repair",
        json!({"still":"wrong type"}),
    )
    .await
    .unwrap();
    assert!(matches!(
        worker
            .replay_once(&processor, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayFailed { .. }
    ));
    assert_eq!(processor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(worker.quarantined_inputs().await.unwrap().len(), 1);
}

fn control_request(
    command: StreamStageCommand,
    revision: u64,
    reset: bool,
) -> StreamStageControlRequest {
    StreamStageControlRequest {
        request_id: Uuid::new_v4(),
        command,
        expected_control_revision: revision,
        reason: "operator maintenance".into(),
        reset_retry_budget: reset,
    }
}

#[tokio::test]
async fn audited_halt_resume_is_durable_idempotent_and_revision_guarded() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let halt = control_request(StreamStageCommand::Halt, 0, false);
    let audit = op.control("operator", &halt).await.unwrap();
    assert_eq!(op.control("operator", &halt).await.unwrap(), audit);
    assert!(matches!(
        op.control("other", &halt).await,
        Err(StreamStageError::ControlConflict)
    ));
    let mut source = Source::new(store.clone(), 0..1);
    let p = Processor::new();
    assert!(matches!(
        worker
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { .. }
    ));
    assert_eq!(source.receive_calls, 0);
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    let mut restarted = stage(store, StreamStageConfig::default()).await;
    assert!(restarted.metrics().await.unwrap().operator_halted);
    let stale = control_request(StreamStageCommand::Resume, 0, false);
    assert!(matches!(
        op.control("operator", &stale).await,
        Err(StreamStageError::ControlConflict)
    ));
    let resume = control_request(StreamStageCommand::Resume, 1, false);
    let resumed = op.control("operator", &resume).await.unwrap();
    op.control(
        "operator",
        &control_request(StreamStageCommand::Halt, 2, false),
    )
    .await
    .unwrap();
    assert_eq!(op.control("operator", &resume).await.unwrap(), resumed);
    assert!(op.status().operator_halted);
    assert_eq!(op.status().checkpoint_generation, 0);
    assert_eq!(op.status().control_revision, 3);
    op.control(
        "operator",
        &control_request(StreamStageCommand::Resume, 3, false),
    )
    .await
    .unwrap();
    assert!(matches!(
        restarted
            .process_once(&mut source, &p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Completed { .. }
    ));
    assert_eq!(source.ack_calls, 1);
}

#[tokio::test]
async fn explicit_resume_reset_retains_failed_source_anchor_and_new_budget() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let config = StreamStageConfig {
        max_attempts: 1,
        ..Default::default()
    };
    let mut worker = stage(store.clone(), config.clone()).await;
    let mut source = Source::new(store.clone(), 0..1);
    let failure = Processor {
        failures: AtomicUsize::new(1),
        permanent: true,
        ..Processor::new()
    };
    assert!(matches!(
        worker
            .process_once(&mut source, &failure, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { .. }
    ));
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.control(
        "operator",
        &control_request(StreamStageCommand::Resume, 0, false),
    )
    .await
    .unwrap();
    assert!(worker.metrics().await.unwrap().halted);
    let audit = op
        .control(
            "operator",
            &control_request(StreamStageCommand::Resume, 1, true),
        )
        .await
        .unwrap();
    assert_eq!(audit.previous_retry_attempts, Some(1));
    assert!(audit.previous_retry_terminal);
    let mut worker = stage(store.clone(), config).await;
    let p = Processor::new();
    let mut skipping = Source::new(store, 1..2);
    assert!(matches!(
        worker
            .process_once(&mut skipping, &p, &CancellationToken::new())
            .await,
        Err(StreamStageError::RetryAnchorMissing)
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    worker
        .process_once(&mut source, &p, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(worker.metrics().await.unwrap().counters.attempts, 2);
    assert_eq!(*worker.checkpoint().state(), 1);
}

#[tokio::test]
async fn halt_fences_running_callback_and_blocks_replay_without_removing_quarantine() {
    let (store, mut worker, id) = replay_fixture(consume_config()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.request_replay(Uuid::new_v4(), &id, "operator", "repair", json!(7))
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let processor = BlockReplay {
        entered: entered.clone(),
        release: release.clone(),
    };
    let old = tokio::spawn(async move {
        worker
            .replay_once(&processor, &CancellationToken::new())
            .await
    });
    entered.notified().await;
    let audit = op
        .control(
            "operator",
            &control_request(StreamStageCommand::Halt, 0, false),
        )
        .await
        .unwrap();
    assert!(audit.lease_fenced);
    release.notify_one();
    assert!(matches!(old.await.unwrap(), Err(StreamStageError::Fenced)));
    let mut replacement = stage(store, consume_config()).await;
    let p = Processor::new();
    assert!(matches!(
        replacement
            .replay_once(&p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Halted { .. }
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    assert_eq!(op.quarantined_inputs().len(), 1);
    assert_eq!(*replacement.checkpoint().state(), 0);
    assert!(replacement.checkpoint().pending_outputs().is_empty());
    op.control(
        "operator",
        &control_request(StreamStageCommand::Resume, 1, false),
    )
    .await
    .unwrap();
    assert!(matches!(
        replacement
            .replay_once(&p, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::ReplayCompleted { .. }
    ));
}

#[tokio::test]
async fn control_history_is_bounded_without_evicting_idempotency() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    stage(store.clone(), StreamStageConfig::default()).await;
    let mut op = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    for revision in 0..999 {
        op.control(
            "operator",
            &control_request(StreamStageCommand::Halt, revision, false),
        )
        .await
        .unwrap();
    }
    let old = op.control_audits()[0].clone();
    assert!(matches!(
        op.control(
            "operator",
            &control_request(StreamStageCommand::Halt, 999, false)
        )
        .await,
        Err(StreamStageError::ControlCapacity)
    ));
    // The reserved final slot can always release the operator hold.
    op.control(
        "operator",
        &control_request(StreamStageCommand::Resume, 999, false),
    )
    .await
    .unwrap();
    assert!(matches!(
        op.control(
            "operator",
            &control_request(StreamStageCommand::Resume, 1000, false)
        )
        .await,
        Err(StreamStageError::ControlCapacity)
    ));
    assert!(!op.status().operator_halted);
    assert_eq!(op.control("operator", &old.request).await.unwrap(), old);
}

#[tokio::test]
async fn lost_control_commit_response_is_recovered_without_reapplying_command() {
    let store: Arc<dyn StateStore> = Arc::new(LostWriteReply {
        inner: MemoryStateStore::new(),
        lost: std::sync::atomic::AtomicBool::new(false),
        replay_completion: false,
        control_completion: true,
    });
    stage(store.clone(), StreamStageConfig::default()).await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let halt = control_request(StreamStageCommand::Halt, 0, false);
    assert!(op.control("operator", &halt).await.is_err());
    let mut op = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    assert!(op.status().operator_halted);
    let audit = op.control("operator", &halt).await.unwrap();
    assert_eq!(audit.control_revision, 1);
    op.control(
        "operator",
        &control_request(StreamStageCommand::Resume, 1, false),
    )
    .await
    .unwrap();
    assert_eq!(op.control("operator", &halt).await.unwrap(), audit);
    assert!(!op.status().operator_halted);
    assert_eq!(op.control_audits().len(), 2);
}

#[tokio::test]
async fn halt_fences_normal_input_commit_and_acknowledgement() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let processor = BlockReplay {
        entered: entered.clone(),
        release: release.clone(),
    };
    let task_store = store.clone();
    let task = tokio::spawn(async move {
        let mut source = Source::new(task_store, 0..1);
        let result = worker
            .process_once(&mut source, &processor, &CancellationToken::new())
            .await;
        (result, source.ack_calls)
    });
    entered.notified().await;
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.control(
        "operator",
        &control_request(StreamStageCommand::Halt, 0, false),
    )
    .await
    .unwrap();
    release.notify_one();
    let (result, acks) = task.await.unwrap();
    assert!(matches!(result, Err(StreamStageError::Fenced)));
    assert_eq!(acks, 0);
    let restored = stage(store, StreamStageConfig::default()).await;
    assert_eq!(*restored.checkpoint().state(), 0);
    assert!(restored.checkpoint().pending_outputs().is_empty());
    assert!(restored.checkpoint().positions().is_empty());
}

#[tokio::test]
async fn controls_require_version_six_and_reject_corrupt_audit_revision() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    stage(store.clone(), StreamStageConfig::default()).await;
    let original: serde_json::Value =
        serde_json::from_str(&store.get(&key()).await.unwrap().unwrap()).unwrap();
    assert_eq!(original["schema_version"], 3);
    let mut op = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    op.control(
        "operator",
        &control_request(StreamStageCommand::Halt, 0, false),
    )
    .await
    .unwrap();
    let mut saved: serde_json::Value =
        serde_json::from_str(&store.get(&key()).await.unwrap().unwrap()).unwrap();
    assert_eq!(saved["schema_version"], 6);
    saved["schema_version"] = json!(5);
    store.set(&key(), &saved.to_string(), None).await.unwrap();
    assert!(
        StreamStageOperator::load(store.clone(), key())
            .await
            .is_err()
    );
    saved["schema_version"] = json!(6);
    saved["processing"]["controls"][0]["control_revision"] = json!(9);
    store.set(&key(), &saved.to_string(), None).await.unwrap();
    assert!(StreamStageOperator::load(store, key()).await.is_err());
}

#[tokio::test]
async fn competing_controls_and_resume_preserve_scheduled_retry_backoff() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let mut worker = stage(store.clone(), StreamStageConfig::default()).await;
    let mut source = Source::new(store.clone(), 0..1);
    let failure = Processor {
        failures: AtomicUsize::new(1),
        ..Processor::new()
    };
    worker
        .process_once(&mut source, &failure, &CancellationToken::new())
        .await
        .unwrap();
    let before = worker.metrics().await.unwrap();
    let mut first = StreamStageOperator::load(store.clone(), key())
        .await
        .unwrap()
        .unwrap();
    let mut second = StreamStageOperator::load(store, key())
        .await
        .unwrap()
        .unwrap();
    let halt = control_request(StreamStageCommand::Halt, 0, false);
    let resume = control_request(StreamStageCommand::Resume, 0, false);
    let (a, b) = tokio::join!(
        first.control("operator", &halt),
        second.control("operator", &resume)
    );
    assert!(a.is_ok());
    assert!(matches!(b, Err(StreamStageError::ControlConflict)));
    first
        .control(
            "operator",
            &control_request(StreamStageCommand::Resume, 1, false),
        )
        .await
        .unwrap();
    let after = worker.metrics().await.unwrap();
    assert_eq!(before.next_attempt_at, after.next_attempt_at);
    assert_eq!(before.counters, after.counters);
    assert_eq!(before.checkpoint_generation, after.checkpoint_generation);
    assert!(!after.halted);
}
