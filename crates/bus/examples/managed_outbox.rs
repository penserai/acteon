//! Run with: cargo run -p acteon-bus --example managed_outbox
use std::sync::Arc;
use std::time::Duration;

use acteon_bus::{
    BusBackend, BusMessage, BusOutboxDelivery, MemoryBackend, ScanFrom, StreamCheckpointConfig,
    StreamCheckpointCoordinator, StreamOutboxConfig, StreamOutboxDispatchResult,
    StreamOutboxDispatcher, StreamOutboxEntry, stream_checkpoint_key,
};
use acteon_core::Topic;
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use chrono::Utc;
use futures::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let bus = MemoryBackend::new();
    let topic = Topic::new("operations", "acme", "decisions");
    let topic_name = topic.kafka_topic_name();
    let key = stream_checkpoint_key("operations", "acme", "detector");
    let mut checkpoint = StreamCheckpointCoordinator::initialize(
        store.clone(),
        key.clone(),
        json!({"batch": 0}),
        StreamCheckpointConfig::default(),
    )
    .await?;
    checkpoint
        .checkpoint(
            json!({"batch": 1}),
            [],
            [StreamOutboxEntry {
                idempotency_key: "checkout:window-42".into(),
                created_at: Utc::now(),
                payload: BusMessage::new(
                    &topic_name,
                    json!({"incident": "latency", "severity": "high"}),
                ),
            }],
        )
        .await?;
    let config = StreamOutboxConfig {
        initial_backoff_ms: 10,
        max_backoff_ms: 10,
        ..Default::default()
    };
    let delivery = BusOutboxDelivery::new(bus.clone());
    let mut worker =
        StreamOutboxDispatcher::initialize(checkpoint, "worker-1", config.clone()).await?;
    // The receiver topic is absent: a real bus invocation fails and is retried.
    let first = worker.dispatch_once(&delivery).await?;
    assert!(matches!(
        first,
        StreamOutboxDispatchResult::RetryScheduled { .. }
    ));
    println!("Missing receiver: {first:?}");
    drop(worker);

    bus.create_topic(&topic).await?;
    tokio::time::sleep(Duration::from_millis(15)).await;
    let restored = StreamCheckpointCoordinator::<serde_json::Value, BusMessage>::initialize(
        store,
        key,
        json!({}),
        StreamCheckpointConfig::default(),
    )
    .await?;
    let mut worker = StreamOutboxDispatcher::initialize(restored, "worker-2", config).await?;
    let second = worker.dispatch_once(&delivery).await?;
    assert!(matches!(
        second,
        StreamOutboxDispatchResult::Delivered { .. }
    ));
    println!("After worker restart: {second:?}");
    let mut messages = bus.scan_topic(&topic_name, ScanFrom::Earliest).await?;
    let received = messages.next().await.expect("published message")?;
    assert_eq!(received.headers["idempotency-key"], "checkout:window-42");
    println!(
        "Receiver: {} (key={})",
        received.payload, received.headers["idempotency-key"]
    );
    let metrics = worker.metrics().await?;
    assert_eq!(metrics.pending, 0);
    assert_eq!(metrics.counters.attempts, 2);
    assert_eq!(metrics.counters.delivered, 1);
    assert_eq!(metrics.counters.retries_scheduled, 1);
    println!("Metrics: {metrics:?}");
    Ok(())
}
