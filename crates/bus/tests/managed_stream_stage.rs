//! Managed-stage recovery through native Kafka receipt capabilities.
use acteon_bus::{
    BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, LiveStreamStageSource,
    ManagedStreamStage, StartOffset, StreamCheckpointConfig, StreamCheckpointCoordinator,
    StreamOutboxEntry, StreamStageConfig, StreamStageInput, StreamStageProcessError,
    StreamStageProcessor, StreamStageResult, StreamStageSource, StreamStageSubscription,
    StreamStageTransition, SubscriptionConfig, stream_checkpoint_key,
};
use acteon_core::Topic;
use acteon_state_memory::MemoryStateStore;
use async_trait::async_trait;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

struct Sum(AtomicUsize);
#[async_trait]
impl StreamStageProcessor<u64, u64, u64> for Sum {
    async fn process(
        &self,
        state: u64,
        inputs: &[StreamStageInput<u64>],
    ) -> Result<StreamStageTransition<u64, u64>, StreamStageProcessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(StreamStageTransition {
            state: state + inputs.iter().map(|i| i.payload).sum::<u64>(),
            outputs: inputs
                .iter()
                .map(|i| StreamOutboxEntry {
                    idempotency_key: format!("offset:{}", i.position.offset),
                    created_at: chrono::Utc::now(),
                    payload: i.payload,
                })
                .collect(),
        })
    }
}

#[tokio::test]
async fn native_stage_replays_saved_positions_without_repeating_processing() {
    let Ok(bootstrap_servers) = std::env::var("ACTEON_KAFKA_BOOTSTRAP") else {
        return;
    };
    let backend = KafkaBackend::new(&KafkaBusConfig {
        bootstrap_servers,
        ..Default::default()
    })
    .unwrap();
    let mut topic = Topic::new(
        format!("stage-{}", uuid::Uuid::new_v4().simple()),
        "test",
        "tenant",
    );
    topic.partitions = 1;
    topic.replication_factor = 1;
    backend.create_topic(&topic).await.unwrap();
    let name = topic.kafka_topic_name();
    let group = format!("stage-{}", uuid::Uuid::new_v4());
    backend
        .produce(BusMessage::new(&name, serde_json::json!(7)))
        .await
        .unwrap();
    let definitions = vec![StreamStageSubscription::new(
        "input",
        &name,
        &group,
        StartOffset::Earliest,
    )];
    let mut source = LiveStreamStageSource::connect(
        backend.clone(),
        definitions.clone(),
        SubscriptionConfig::default(),
    )
    .unwrap();
    let store = Arc::new(MemoryStateStore::new());
    let key = stream_checkpoint_key("test", "tenant", &group);
    let coordinator = StreamCheckpointCoordinator::initialize(
        store.clone(),
        key.clone(),
        0_u64,
        StreamCheckpointConfig::default(),
    )
    .await
    .unwrap();
    let config = StreamStageConfig {
        receive_timeout_ms: 25_000,
        lease_ms: 90_000,
        ..Default::default()
    };
    let mut stage = ManagedStreamStage::initialize(
        coordinator,
        "original",
        "sum-v1",
        source.identity(),
        config.clone(),
    )
    .await
    .unwrap();
    let processor = Sum(AtomicUsize::new(0));
    let cancel = CancellationToken::new();
    // A wrapper cancels after real receipt validation, before input acknowledgement.
    struct Pause {
        inner: LiveStreamStageSource,
        cancel: CancellationToken,
    }
    #[async_trait]
    impl StreamStageSource for Pause {
        type Receipt = <LiveStreamStageSource as StreamStageSource>::Receipt;
        fn identity(&self) -> String {
            self.inner.identity()
        }
        async fn receive(
            &mut self,
            max: usize,
        ) -> Result<
            Vec<acteon_bus::StreamStageRecord<Self::Receipt>>,
            acteon_bus::StreamStageSourceError,
        > {
            self.inner.receive(max).await
        }
        async fn validate(
            &mut self,
            records: &[acteon_bus::StreamStageRecord<Self::Receipt>],
        ) -> Result<Vec<acteon_bus::StreamPosition>, acteon_bus::StreamStageSourceError> {
            let positions = self.inner.validate(records).await?;
            self.cancel.cancel();
            Ok(positions)
        }
        async fn acknowledge(
            &mut self,
            _: &[acteon_bus::StreamStageRecord<Self::Receipt>],
        ) -> Result<(), acteon_bus::StreamStageSourceError> {
            panic!("cancelled stage must not acknowledge")
        }
        async fn recover(&mut self) -> Result<(), acteon_bus::StreamStageSourceError> {
            self.inner.recover().await
        }
        async fn close(&mut self) {
            self.inner.close().await;
        }
    }
    let mut paused = Pause {
        inner: source,
        cancel: cancel.clone(),
    };
    assert_eq!(
        stage
            .process_once(&mut paused, &processor, &cancel)
            .await
            .unwrap(),
        StreamStageResult::Cancelled {
            durable_generation: Some(1)
        }
    );
    paused.close().await;
    drop(stage);
    source =
        LiveStreamStageSource::connect(backend.clone(), definitions, SubscriptionConfig::default())
            .unwrap();
    let coordinator = StreamCheckpointCoordinator::initialize(
        store,
        key,
        0_u64,
        StreamCheckpointConfig::default(),
    )
    .await
    .unwrap();
    stage = ManagedStreamStage::initialize(
        coordinator,
        "replacement",
        "sum-v1",
        source.identity(),
        config,
    )
    .await
    .unwrap();
    assert_eq!(
        stage
            .process_once(&mut source, &processor, &CancellationToken::new())
            .await
            .unwrap(),
        StreamStageResult::Completed {
            generation: 1,
            processed: 0,
            recovered: 1
        }
    );
    assert_eq!(processor.0.load(Ordering::SeqCst), 1);
    assert_eq!(*stage.checkpoint().state(), 7);
    assert_eq!(stage.checkpoint().pending_outputs().len(), 1);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let lag = backend.consumer_lag(&name, &group).await.unwrap();
            if lag.iter().all(|p| p.lag == 0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    source.close().await;
    backend.delete_topic(&name).await.unwrap();
}
