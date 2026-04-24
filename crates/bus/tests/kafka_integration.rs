//! Integration test against a real Kafka broker.
//!
//! Runs only when `ACTEON_KAFKA_BOOTSTRAP` is set (e.g. to
//! `localhost:9092` for the docker-compose `kafka` profile). CI and
//! laptop-without-docker users simply skip it.

use std::time::Duration;

use futures::StreamExt;

use acteon_bus::{BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, StartOffset};
use acteon_core::Topic;

fn brokers() -> Option<String> {
    std::env::var("ACTEON_KAFKA_BOOTSTRAP").ok()
}

fn make_backend(client_id: &str) -> std::sync::Arc<KafkaBackend> {
    let cfg = KafkaBusConfig {
        bootstrap_servers: brokers().unwrap(),
        client_id: client_id.to_string(),
        produce_timeout_ms: 8_000,
        extra: Vec::new(),
    };
    KafkaBackend::new(&cfg).expect("kafka backend")
}

fn unique_topic(suffix: &str) -> Topic {
    let name = format!("it{}-{}", uuid::Uuid::new_v4().simple(), suffix);
    let mut t = Topic::new(&name, "test", "tenant");
    t.partitions = 1;
    t.replication_factor = 1;
    t
}

#[tokio::test]
async fn produce_and_subscribe_end_to_end() {
    let Some(_) = brokers() else {
        eprintln!("skipping: ACTEON_KAFKA_BOOTSTRAP not set");
        return;
    };
    let backend = make_backend("acteon-bus-it-prod");
    let topic = unique_topic("e2e");

    backend.create_topic(&topic).await.expect("create topic");

    // Give Kafka a moment to propagate metadata for the new topic.
    tokio::time::sleep(Duration::from_millis(500)).await;

    for i in 0..5 {
        backend
            .produce(
                BusMessage::new(topic.kafka_topic_name(), serde_json::json!({ "n": i }))
                    .with_key("ordering-key"),
            )
            .await
            .expect("produce");
    }

    let group = format!("it-group-{}", uuid::Uuid::new_v4().simple());
    let mut stream = backend
        .subscribe(&topic.kafka_topic_name(), &group, StartOffset::Earliest)
        .await
        .expect("subscribe");

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while seen.len() < 5 {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(msg))) => seen.push(msg.payload["n"].as_i64().unwrap_or(-1)),
            Ok(Some(Err(e))) => panic!("stream error: {e}"),
            Ok(None) | Err(_) => break,
        }
    }
    assert_eq!(seen.len(), 5, "expected 5 messages, got {seen:?}");
    assert_eq!(seen, vec![0, 1, 2, 3, 4], "ordering by shared key");

    backend
        .delete_topic(&topic.kafka_topic_name())
        .await
        .expect("delete");
}

#[tokio::test]
async fn delete_missing_topic_returns_not_found() {
    let Some(_) = brokers() else {
        eprintln!("skipping: ACTEON_KAFKA_BOOTSTRAP not set");
        return;
    };
    let backend = make_backend("acteon-bus-it-missing");
    let err = backend
        .delete_topic("test.tenant.does-not-exist-ever")
        .await
        .unwrap_err();
    assert!(matches!(err, acteon_bus::BusError::TopicNotFound(_)));
}
