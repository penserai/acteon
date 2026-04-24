//! `rdkafka`-backed [`BusBackend`].
//!
//! One `FutureProducer` is shared across all produces. Each subscribe
//! spawns a fresh `StreamConsumer` bound to the supplied `group_id`,
//! because Phase 1 keeps subscriber lifetimes tied to a single SSE
//! connection. Phase 2 promotes subscriptions to a first-class type
//! with persistent consumers.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures::StreamExt;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::{Header, Headers, Message, OwnedHeaders};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;

use acteon_core::Topic;

use crate::backend::{BusBackend, SubscribeStream};
use crate::config::KafkaBusConfig;
use crate::error::BusError;
use crate::message::{BusMessage, DeliveryReceipt, StartOffset};

/// `BusBackend` impl that talks to a real Kafka cluster.
pub struct KafkaBackend {
    producer: FutureProducer,
    admin: AdminClient<DefaultClientContext>,
    bootstrap: String,
    client_id: String,
    produce_timeout: Duration,
    extra: Vec<(String, String)>,
}

impl KafkaBackend {
    /// Build a new backend from config. Does not contact the broker —
    /// connections are lazy on first produce/subscribe/admin call.
    pub fn new(config: &KafkaBusConfig) -> Result<Arc<Self>, BusError> {
        let mut cfg = ClientConfig::new();
        cfg.set("bootstrap.servers", &config.bootstrap_servers);
        cfg.set("client.id", &config.client_id);
        for (k, v) in &config.extra {
            cfg.set(k, v);
        }
        cfg.set("message.timeout.ms", config.produce_timeout_ms.to_string());
        cfg.set("enable.idempotence", "true");

        let producer: FutureProducer = cfg
            .create()
            .map_err(|e| BusError::Transport(format!("producer: {e}")))?;
        let admin: AdminClient<DefaultClientContext> = cfg
            .create()
            .map_err(|e| BusError::Transport(format!("admin: {e}")))?;

        Ok(Arc::new(Self {
            producer,
            admin,
            bootstrap: config.bootstrap_servers.clone(),
            client_id: config.client_id.clone(),
            produce_timeout: Duration::from_millis(config.produce_timeout_ms),
            extra: config.extra.clone(),
        }))
    }

    fn consumer_config(&self, group_id: &str, from: StartOffset) -> ClientConfig {
        let mut cfg = ClientConfig::new();
        cfg.set("bootstrap.servers", &self.bootstrap);
        cfg.set("client.id", &self.client_id);
        cfg.set("group.id", group_id);
        cfg.set(
            "auto.offset.reset",
            match from {
                StartOffset::Earliest => "earliest",
                StartOffset::Latest => "latest",
            },
        );
        // Phase 1 does not commit offsets — callers that reconnect get
        // either the earliest or latest records per `from`.
        cfg.set("enable.auto.commit", "false");
        for (k, v) in &self.extra {
            cfg.set(k, v);
        }
        cfg
    }
}

fn map_kafka_error(err: KafkaError) -> BusError {
    let unknown_topic = |code: RDKafkaErrorCode| -> bool {
        matches!(
            code,
            RDKafkaErrorCode::UnknownTopic | RDKafkaErrorCode::UnknownTopicOrPartition
        )
    };
    match err {
        KafkaError::AdminOp(RDKafkaErrorCode::TopicAlreadyExists)
        | KafkaError::MessageProduction(RDKafkaErrorCode::TopicAlreadyExists) => {
            BusError::TopicAlreadyExists(String::new())
        }
        KafkaError::MessageProduction(code) if unknown_topic(code) => {
            // Produce against a non-existent topic was previously a
            // generic Transport/500; callers get a 404 now, matching
            // the consume path.
            BusError::TopicNotFound(String::new())
        }
        KafkaError::MessageConsumption(code) if unknown_topic(code) => {
            BusError::TopicNotFound(String::new())
        }
        KafkaError::AdminOp(code) | KafkaError::MessageProduction(code) => {
            BusError::Transport(format!("kafka: {code:?}"))
        }
        other => BusError::Transport(other.to_string()),
    }
}

#[async_trait]
impl BusBackend for KafkaBackend {
    async fn create_topic(&self, topic: &Topic) -> Result<(), BusError> {
        let name = topic.kafka_topic_name();
        let new_topic = NewTopic::new(
            &name,
            topic.partitions,
            TopicReplication::Fixed(i32::from(topic.replication_factor)),
        );
        let configured: Vec<(String, String)> = if let Some(ms) = topic.retention_ms {
            vec![("retention.ms".to_string(), ms.to_string())]
        } else {
            Vec::new()
        };
        let new_topic = configured
            .iter()
            .fold(new_topic, |nt, (k, v)| nt.set(k.as_str(), v.as_str()));
        let results = self
            .admin
            .create_topics(&[new_topic], &AdminOptions::new())
            .await
            .map_err(map_kafka_error)?;
        for res in results {
            match res {
                Ok(_) | Err((_, RDKafkaErrorCode::TopicAlreadyExists)) => {}
                Err((topic_name, code)) => {
                    return Err(BusError::Transport(format!(
                        "create_topic {topic_name}: {code:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    async fn delete_topic(&self, kafka_name: &str) -> Result<(), BusError> {
        let results = self
            .admin
            .delete_topics(&[kafka_name], &AdminOptions::new())
            .await
            .map_err(map_kafka_error)?;
        for res in results {
            match res {
                Ok(_) => {}
                Err((_, RDKafkaErrorCode::UnknownTopicOrPartition)) => {
                    return Err(BusError::TopicNotFound(kafka_name.into()));
                }
                Err((_, code)) => {
                    return Err(BusError::Transport(format!("delete_topic: {code:?}")));
                }
            }
        }
        Ok(())
    }

    async fn produce(&self, message: BusMessage) -> Result<DeliveryReceipt, BusError> {
        let BusMessage {
            topic,
            key,
            payload,
            headers,
            ..
        } = message;
        let payload_bytes =
            serde_json::to_vec(&payload).map_err(|e| BusError::Serialization(e.to_string()))?;
        let mut kheaders = OwnedHeaders::new();
        for (k, v) in &headers {
            kheaders = kheaders.insert(Header {
                key: k,
                value: Some(v.as_bytes()),
            });
        }
        let mut record: FutureRecord<'_, str, Vec<u8>> = FutureRecord::to(&topic)
            .payload(&payload_bytes)
            .headers(kheaders);
        if let Some(ref k) = key {
            record = record.key(k.as_str());
        }
        let (partition, offset) = self
            .producer
            .send(record, Timeout::After(self.produce_timeout))
            .await
            .map_err(|(e, _msg)| match e {
                KafkaError::MessageProduction(RDKafkaErrorCode::MessageTimedOut) => {
                    BusError::Timeout
                }
                other => map_kafka_error(other),
            })?;
        Ok(DeliveryReceipt {
            topic,
            partition,
            offset,
            timestamp: Utc::now(),
        })
    }

    async fn subscribe(
        &self,
        kafka_topic: &str,
        group_id: &str,
        from: StartOffset,
    ) -> Result<SubscribeStream, BusError> {
        let cfg = self.consumer_config(group_id, from);
        let consumer: StreamConsumer = cfg
            .create()
            .map_err(|e| BusError::Transport(format!("consumer: {e}")))?;
        // `assign()` (manual partition placement) and `subscribe()`
        // (dynamic consumer-group rebalance) are mutually exclusive in
        // librdkafka. Phase 1 uses `group.id` for everything, so rely
        // purely on `subscribe()`. `auto.offset.reset` in
        // `consumer_config` handles Earliest vs Latest when the group
        // has no committed offset.
        consumer
            .subscribe(&[kafka_topic])
            .map_err(|e| BusError::Transport(format!("subscribe: {e}")))?;

        let topic_owned = kafka_topic.to_string();
        let stream = async_stream::stream! {
            let mut stream = consumer.stream();
            while let Some(res) = stream.next().await {
                match res {
                    Ok(msg) => {
                        let payload = msg.payload().map_or(serde_json::Value::Null, |b| {
                            serde_json::from_slice::<serde_json::Value>(b)
                                .unwrap_or(serde_json::Value::Null)
                        });
                        let key = msg
                            .key()
                            .and_then(|b| std::str::from_utf8(b).ok())
                            .map(str::to_string);
                        let mut headers = std::collections::BTreeMap::new();
                        if let Some(h) = msg.headers() {
                            for i in 0..h.count() {
                                let rec = h.get(i);
                                let name = rec.key.to_string();
                                if let Some(v) = rec.value
                                    && let Ok(s) = std::str::from_utf8(v)
                                {
                                    headers.insert(name, s.to_string());
                                }
                            }
                        }
                        let timestamp = msg
                            .timestamp()
                            .to_millis()
                            .and_then(|ms| Utc.timestamp_millis_opt(ms).single());
                        yield Ok(BusMessage {
                            topic: topic_owned.clone(),
                            key,
                            payload,
                            headers,
                            partition: Some(msg.partition()),
                            offset: Some(msg.offset()),
                            timestamp,
                        });
                    }
                    Err(e) => {
                        yield Err(BusError::Transport(e.to_string()));
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}
