//! Active-consumer acknowledgement conformance. CI supplies a real broker.
use std::sync::Arc;
use std::time::Duration;

use acteon_bus::{
    AcknowledgedSubscription, BusBackend, BusMessage, KafkaBackend, KafkaBusConfig, StartOffset,
    SubscriptionConfig, SubscriptionDelivery, SubscriptionError,
};
use acteon_core::Topic;
use rdkafka::producer::{FutureProducer, FutureRecord};

fn backend() -> Option<Arc<KafkaBackend>> {
    let bootstrap_servers = std::env::var("ACTEON_KAFKA_BOOTSTRAP").ok()?;
    Some(
        KafkaBackend::new(&KafkaBusConfig {
            bootstrap_servers,
            extra: vec![
                ("session.timeout.ms".into(), "6000".into()),
                ("heartbeat.interval.ms".into(), "1000".into()),
                // Deliberately unsafe overrides must not defeat live-session policy.
                ("enable.auto.commit".into(), "true".into()),
                ("enable.auto.offset.store".into(), "true".into()),
                (
                    "partition.assignment.strategy".into(),
                    "cooperative-sticky".into(),
                ),
            ],
            ..Default::default()
        })
        .unwrap(),
    )
}

fn topic(partitions: i32) -> Topic {
    let mut topic = Topic::new(
        format!("ack-{}", uuid::Uuid::new_v4().simple()),
        "test",
        "tenant",
    );
    topic.partitions = partitions;
    topic.replication_factor = 1;
    topic
}

async fn receive(subscription: &mut dyn AcknowledgedSubscription) -> SubscriptionDelivery {
    tokio::time::timeout(Duration::from_secs(25), subscription.recv())
        .await
        .expect("delivery deadline")
        .expect("delivery")
}

#[tokio::test]
async fn live_acknowledgements_wait_for_gaps_and_resume_after_broker_confirmed_prefix() {
    let Some(backend) = backend() else {
        return;
    };
    let topic = topic(1);
    let name = topic.kafka_topic_name();
    let group = format!("ack-{}", uuid::Uuid::new_v4());
    backend.create_topic(&topic).await.unwrap();
    for n in 0..3 {
        backend
            .produce(BusMessage::new(&name, serde_json::json!({"n": n})))
            .await
            .unwrap();
    }
    let scoped_backend = KafkaBackend::new(&KafkaBusConfig {
        bootstrap_servers: std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap(),
        extra: vec![
            ("group.id".into(), format!("wrong-{group}")),
            ("enable.auto.commit".into(), "true".into()),
            ("enable.auto.offset.store".into(), "true".into()),
        ],
        ..Default::default()
    })
    .unwrap();
    let mut session = scoped_backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig { max_in_flight: 2 },
        )
        .await
        .unwrap();
    let first = receive(session.as_mut()).await;
    assert_eq!(first.receipt.consumer_group(), group);
    let second = receive(session.as_mut()).await;
    assert!(matches!(
        session.recv().await,
        Err(SubscriptionError::Capacity { limit: 2 })
    ));
    let gap = session
        .acknowledge(&[second.receipt.clone()])
        .await
        .unwrap();
    assert!(gap.committed.is_empty());
    let lag = backend.consumer_lag(&name, &group).await.unwrap();
    assert_eq!(
        lag[0].committed, -1,
        "unsafe auto-commit override must be suppressed"
    );
    let prefix = session.acknowledge(&[first.receipt.clone()]).await.unwrap();
    assert_eq!(prefix.committed[0].offset, 1);
    assert_eq!(prefix.remaining_in_flight, 0);
    assert!(
        session
            .acknowledge(&[first.receipt])
            .await
            .unwrap()
            .committed
            .is_empty()
    );
    drop(session);
    let mut replacement = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    let third = receive(replacement.as_mut()).await;
    assert_eq!(third.message.payload["n"], 2);
    replacement.acknowledge(&[third.receipt]).await.unwrap();
    assert_eq!(backend.consumer_lag(&name, &group).await.unwrap()[0].lag, 0);
    drop(replacement);
    backend.delete_topic(&name).await.unwrap();
}

#[tokio::test]
async fn rebalance_fences_old_receipts_and_redelivers_to_the_current_members() {
    let Some(backend) = backend() else {
        return;
    };
    let topic = topic(2);
    let name = topic.kafka_topic_name();
    let group = format!("rebalance-{}", uuid::Uuid::new_v4());
    backend.create_topic(&topic).await.unwrap();
    let producer: FutureProducer = rdkafka::config::ClientConfig::new()
        .set(
            "bootstrap.servers",
            std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap(),
        )
        .create()
        .unwrap();
    for partition in 0..2 {
        for n in 0..8 {
            let payload = serde_json::json!({"n":n,"partition":partition}).to_string();
            producer
                .send(
                    FutureRecord::<(), _>::to(&name)
                        .partition(partition)
                        .payload(&payload),
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
        }
    }
    let mut first = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    let old = receive(first.as_mut()).await;
    let mut ownership = first.ownership_changes();
    let original_epoch = ownership.borrow_and_update().epoch;
    assert_eq!(ownership.borrow().partitions.len(), 2);
    let mut second = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    let first_poll = async {
        loop {
            // Both consumers must poll to serve their rebalance callbacks.
            let _ = tokio::time::timeout(Duration::from_millis(200), first.recv()).await;
            if ownership.borrow().epoch != original_epoch {
                break;
            }
        }
    };
    let (_, replacement) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(first_poll, receive(second.as_mut()))
    })
    .await
    .expect("rebalance deadline");
    assert!(matches!(
        first.validate_receipts(&[old.receipt.clone()]),
        Err(SubscriptionError::StaleReceipt)
    ));
    assert!(matches!(
        first.acknowledge(&[old.receipt]).await,
        Err(SubscriptionError::StaleReceipt)
    ));
    assert_eq!(
        replacement.receipt.position().offset,
        0,
        "uncommitted records redeliver"
    );
    second
        .acknowledge(&[replacement.receipt.clone()])
        .await
        .unwrap();
    assert!(matches!(
        first.acknowledge(&[replacement.receipt]).await,
        Err(SubscriptionError::WrongSubscription)
    ));
    drop(first);
    drop(second);
    backend.delete_topic(&name).await.unwrap();
}

#[tokio::test]
async fn malformed_json_closes_the_session_without_committing_past_the_record() {
    let Some(backend) = backend() else {
        return;
    };
    let topic = topic(1);
    let name = topic.kafka_topic_name();
    let group = format!("poison-{}", uuid::Uuid::new_v4());
    backend.create_topic(&topic).await.unwrap();
    let producer: FutureProducer = rdkafka::config::ClientConfig::new()
        .set(
            "bootstrap.servers",
            std::env::var("ACTEON_KAFKA_BOOTSTRAP").unwrap(),
        )
        .create()
        .unwrap();
    producer
        .send(
            FutureRecord::<(), _>::to(&name).payload("{broken-json"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    backend
        .produce(BusMessage::new(&name, serde_json::json!({"valid":true})))
        .await
        .unwrap();
    let mut session = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(25), session.recv())
        .await
        .unwrap();
    assert!(matches!(outcome, Err(SubscriptionError::InvalidRecord(_))));
    assert!(matches!(
        session.recv().await,
        Err(SubscriptionError::Closed)
    ));
    assert!(matches!(
        session.acknowledge(&[]).await,
        Err(SubscriptionError::Closed)
    ));
    assert_eq!(
        backend.consumer_lag(&name, &group).await.unwrap()[0].committed,
        -1
    );
    drop(session);
    backend.delete_topic(&name).await.unwrap();
}

#[tokio::test]
async fn exceeding_the_poll_interval_fences_the_idle_consumers_receipt() {
    let Some(bootstrap_servers) = std::env::var("ACTEON_KAFKA_BOOTSTRAP").ok() else {
        return;
    };
    let backend = KafkaBackend::new(&KafkaBusConfig {
        bootstrap_servers,
        extra: vec![
            ("max.poll.interval.ms".into(), "6000".into()),
            ("session.timeout.ms".into(), "6000".into()),
        ],
        ..Default::default()
    })
    .unwrap();
    let topic = topic(1);
    let name = topic.kafka_topic_name();
    let group = format!("idle-{}", uuid::Uuid::new_v4());
    backend.create_topic(&topic).await.unwrap();
    backend
        .produce(BusMessage::new(&name, serde_json::json!({"n":0})))
        .await
        .unwrap();
    let mut session = backend
        .subscribe_acknowledged(
            &name,
            &group,
            StartOffset::Earliest,
            SubscriptionConfig::default(),
        )
        .await
        .unwrap();
    let delivery = receive(session.as_mut()).await;
    tokio::time::sleep(Duration::from_millis(7500)).await;
    assert!(matches!(
        session.acknowledge(&[delivery.receipt]).await,
        Err(SubscriptionError::StaleReceipt)
    ));
    assert_eq!(
        backend.consumer_lag(&name, &group).await.unwrap()[0].committed,
        -1
    );
    drop(session);
    backend.delete_topic(&name).await.unwrap();
}
