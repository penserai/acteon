//! Socket-level recovery tests using librdkafka's local mock broker.
//! Real-broker conformance remains in `kafka_integration`.

use std::time::Duration;

use acteon_bus::{BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, ScanFrom, StartOffset};
use futures::StreamExt;
use rdkafka::mocking::MockCluster;

async fn recovers_after_disconnect(scan: bool) {
    let cluster = MockCluster::new(1).expect("mock broker");
    let topic = "transport-recovery";
    cluster.create_topic(topic, 1, 1).expect("create topic");
    let backend = KafkaBackend::new(&KafkaBusConfig {
        bootstrap_servers: cluster.bootstrap_servers(),
        extra: vec![
            ("reconnect.backoff.ms".into(), "50".into()),
            ("reconnect.backoff.max.ms".into(), "100".into()),
        ],
        ..Default::default()
    })
    .expect("backend");
    backend
        .produce(BusMessage::new(topic, serde_json::json!({ "n": 0 })))
        .await
        .expect("seed record");
    let mut stream = if scan {
        backend.scan_topic(topic, ScanFrom::Earliest).await
    } else {
        backend
            .subscribe(topic, "recovery", StartOffset::Earliest)
            .await
    }
    .expect("consumer");
    let first = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("first record timeout")
        .expect("stream open")
        .expect("first record");
    assert_eq!(first.payload["n"], 0);

    cluster.broker_down(1).expect("disconnect broker");
    // Poll through the actual socket disconnect. The stream must remain pending
    // while librdkafka reconnects, rather than yielding a terminal BusError.
    assert!(
        tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .is_err(),
        "temporary broker loss must not terminate the stream"
    );
    cluster.broker_up(1).expect("restore broker");
    backend
        .produce(BusMessage::new(topic, serde_json::json!({ "n": 1 })))
        .await
        .expect("record after reconnect");
    let next = tokio::time::timeout(Duration::from_secs(15), stream.next())
        .await
        .expect("recovery timeout")
        .expect("original stream still open")
        .expect("record after reconnect");
    assert_eq!(next.payload["n"], 1);
    assert_eq!(next.offset, Some(1));
}

#[tokio::test]
async fn subscription_survives_broker_disconnect() {
    recovers_after_disconnect(false).await;
}

#[tokio::test]
async fn scan_survives_broker_disconnect() {
    recovers_after_disconnect(true).await;
}
