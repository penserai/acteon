//! Shared conformance checks for [`crate::BusBackend`] implementations.
//!
//! Backend crates run [`run_bus_backend_conformance_tests`] with a fresh,
//! single-partition topic. The contract concentrates on semantics callers can
//! rely on across the in-memory and Kafka transports: typed duplicate-topic
//! errors, ordered replay, message-envelope preservation, offsets, consumer
//! lag, and cursor-based scans.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt;

use acteon_core::Topic;

use crate::{BusBackend, BusError, BusMessage, OffsetPosition, ScanFrom, StartOffset};

const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);

async fn next_message(stream: &mut crate::SubscribeStream, operation: &str) -> crate::BusMessage {
    match tokio::time::timeout(RECEIVE_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(message))) => message,
        Ok(Some(Err(error))) => panic!("{operation}: stream error: {error}"),
        Ok(None) => panic!("{operation}: stream ended before yielding a message"),
        Err(timeout_error) => {
            panic!("{operation}: timed out after {RECEIVE_TIMEOUT:?}: {timeout_error}")
        }
    }
}

async fn assert_replay<B>(backend: &B, topic_name: &str)
where
    B: BusBackend + ?Sized,
{
    let replay_group = format!("acteon-bus-contract-replay-{}", uuid::Uuid::new_v4());
    let mut replay = backend
        .subscribe(topic_name, &replay_group, StartOffset::Earliest)
        .await
        .expect("subscribe from earliest");
    let replayed_first = next_message(&mut replay, "earliest replay first message").await;
    let replayed_second = next_message(&mut replay, "earliest replay second message").await;
    assert_eq!(replayed_first.payload["sequence"], 0);
    assert_eq!(
        replayed_first.key.as_deref(),
        Some("conformance-ordering-key")
    );
    assert_eq!(
        replayed_first.headers.get("x-conformance"),
        Some(&"first".to_string())
    );
    assert_eq!(replayed_first.partition, Some(0));
    assert_eq!(replayed_first.offset, Some(0));
    assert!(replayed_first.timestamp.is_some());
    assert_eq!(replayed_second.payload["sequence"], 1);
    assert_eq!(replayed_second.offset, Some(1));
}

async fn assert_scan<B>(backend: &B, topic_name: &str)
where
    B: BusBackend + ?Sized,
{
    let watermarks = backend
        .scan_topic_watermarks(topic_name)
        .await
        .expect("capture scan watermarks");
    assert_eq!(watermarks.high_water_marks.get(&0), Some(&2));

    let mut offsets = BTreeMap::new();
    offsets.insert(0, 0);
    let mut resumed = backend
        .scan_topic(topic_name, ScanFrom::FromOffsets(offsets))
        .await
        .expect("scan from an explicit offset");
    let resumed_message = next_message(&mut resumed, "cursor scan").await;
    assert_eq!(resumed_message.payload["sequence"], 1);
    assert_eq!(resumed_message.offset, Some(1));
}

async fn assert_lag<B>(backend: &B, topic_name: &str)
where
    B: BusBackend + ?Sized,
{
    let lag_group = format!("acteon-bus-contract-lag-{}", uuid::Uuid::new_v4());
    let before_commit = backend
        .consumer_lag(topic_name, &lag_group)
        .await
        .expect("read lag before commit");
    assert_eq!(before_commit.len(), 1);
    assert_eq!(before_commit[0].partition, 0);
    assert_eq!(before_commit[0].committed, -1);
    assert_eq!(before_commit[0].high_water_mark, 2);
    assert_eq!(before_commit[0].lag, 2);

    backend
        .commit_offset(
            topic_name,
            &lag_group,
            OffsetPosition {
                partition: 0,
                offset: 0,
            },
        )
        .await
        .expect("commit first message offset");
    let after_commit = backend
        .consumer_lag(topic_name, &lag_group)
        .await
        .expect("read lag after commit");
    assert_eq!(after_commit.len(), 1);
    assert_eq!(after_commit[0].partition, 0);
    assert_eq!(after_commit[0].committed, 0);
    assert_eq!(after_commit[0].high_water_mark, 2);
    assert_eq!(after_commit[0].lag, 1);
}

/// Run the portable `BusBackend` contract against one fresh topic.
///
/// `topic` must have exactly one partition. Supplying a unique topic for each
/// invocation keeps the contract safe to run against a shared Kafka broker.
/// The function deletes that topic once the assertions finish.
pub async fn run_bus_backend_conformance_tests<B>(backend: &B, topic: Topic)
where
    B: BusBackend + ?Sized,
{
    assert_eq!(
        topic.partitions, 1,
        "bus conformance requires a single-partition topic"
    );
    let topic_name = topic.kafka_topic_name();

    backend
        .create_topic(&topic)
        .await
        .expect("create conformance topic");
    // Kafka's topic controller acknowledges creation before every broker has
    // refreshed metadata. Give the fresh topic a brief chance to propagate so
    // the portable assertions exercise bus semantics rather than that startup
    // race. This is a no-op from the caller's perspective and keeps the same
    // contract usable against a shared single-node development broker.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let duplicate = backend
        .create_topic(&topic)
        .await
        .expect_err("duplicate topic create must fail");
    assert!(
        matches!(duplicate, BusError::TopicAlreadyExists(ref name) if name == &topic_name),
        "duplicate topic create must return TopicAlreadyExists for {topic_name}, got {duplicate:?}"
    );
    // The rejected request still reaches Kafka's controller. Wait for its
    // metadata update before creating a consumer that resolves the topic.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let first = BusMessage::new(topic_name.clone(), serde_json::json!({ "sequence": 0 }))
        .with_key("conformance-ordering-key")
        .with_header("x-conformance", "first");
    let second = BusMessage::new(topic_name.clone(), serde_json::json!({ "sequence": 1 }))
        .with_key("conformance-ordering-key")
        .with_header("x-conformance", "second");
    let first_receipt = backend.produce(first).await.expect("produce first message");
    let second_receipt = backend
        .produce(second)
        .await
        .expect("produce second message");
    assert_eq!(first_receipt.topic, topic_name);
    assert_eq!(first_receipt.partition, 0);
    assert_eq!(first_receipt.offset, 0);
    assert_eq!(second_receipt.partition, 0);
    assert_eq!(second_receipt.offset, 1);

    assert_replay(backend, &topic_name).await;
    assert_scan(backend, &topic_name).await;
    assert_lag(backend, &topic_name).await;

    backend
        .delete_topic(&topic_name)
        .await
        .expect("delete conformance topic");
}
