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
async fn commit_and_lag_survive_reconnect() {
    let Some(_) = brokers() else {
        eprintln!("skipping: ACTEON_KAFKA_BOOTSTRAP not set");
        return;
    };
    use acteon_bus::{BusBackend, BusMessage, OffsetPosition, StartOffset};
    let backend = make_backend("acteon-bus-it-commit");
    let topic = unique_topic("commit");
    backend.create_topic(&topic).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let name = topic.kafka_topic_name();
    let group = format!("g-{}", uuid::Uuid::new_v4().simple());

    // Seed 5 records, consume 3, commit, then reconnect — the new
    // consumer should only see records 3 and 4.
    for i in 0..5 {
        backend
            .produce(BusMessage::new(name.clone(), serde_json::json!({ "n": i })))
            .await
            .unwrap();
    }

    // Commit offset 2 out-of-band (records 0..=2 consumed).
    backend
        .commit_offset(
            &name,
            &group,
            OffsetPosition {
                partition: 0,
                offset: 2,
            },
        )
        .await
        .unwrap();

    // Fresh consumer in the same group; use Latest so auto.offset.reset
    // doesn't interfere — since we committed, Kafka uses that.
    let mut stream = backend
        .subscribe(&name, &group, StartOffset::Latest)
        .await
        .unwrap();

    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while seen.len() < 2 {
        match tokio::time::timeout_at(deadline, futures::StreamExt::next(&mut stream)).await {
            Ok(Some(Ok(msg))) => seen.push(msg.payload["n"].as_i64().unwrap_or(-1)),
            _ => break,
        }
    }
    assert_eq!(
        seen,
        vec![3, 4],
        "reconnected consumer should resume after committed offset"
    );

    // Lag should now report the correct high-water mark and committed
    // offset (last consumed = 2, so lag = 2 = records 3, 4).
    let lag = backend.consumer_lag(&name, &group).await.unwrap();
    assert!(!lag.is_empty());
    let p0 = lag.iter().find(|e| e.partition == 0).expect("p0");
    assert_eq!(p0.committed, 2);
    assert_eq!(p0.high_water_mark, 5);
    assert_eq!(p0.lag, 2);

    backend.delete_topic(&name).await.ok();
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
