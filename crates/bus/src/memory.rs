//! In-memory [`BusBackend`] for unit tests.
//!
//! Each topic is a `VecDeque<BusMessage>` under a mutex plus a tokio
//! broadcast channel for live subscribers. Offsets are assigned
//! monotonically per-topic starting at 0. There is only one "partition"
//! (0) — Phase 1 doesn't need multi-partition semantics here, and
//! the `BusBackend` contract doesn't require it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use acteon_core::Topic;

use crate::backend::{BusBackend, SubscribeStream};
use crate::error::BusError;
use crate::message::{BusMessage, DeliveryReceipt, StartOffset};

struct TopicState {
    log: VecDeque<BusMessage>,
    tx: broadcast::Sender<BusMessage>,
    next_offset: i64,
}

/// In-memory backend suitable for unit tests.
#[derive(Default)]
pub struct MemoryBackend {
    topics: Mutex<HashMap<String, TopicState>>,
}

impl MemoryBackend {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Test helper: count retained records in a topic's log.
    #[must_use]
    pub fn log_len(&self, topic: &str) -> usize {
        self.topics.lock().get(topic).map_or(0, |s| s.log.len())
    }
}

#[async_trait]
impl BusBackend for MemoryBackend {
    async fn create_topic(&self, topic: &Topic) -> Result<(), BusError> {
        let name = topic.kafka_topic_name();
        let mut topics = self.topics.lock();
        if topics.contains_key(&name) {
            return Ok(()); // idempotent
        }
        let (tx, _) = broadcast::channel(1024);
        topics.insert(
            name,
            TopicState {
                log: VecDeque::new(),
                tx,
                next_offset: 0,
            },
        );
        Ok(())
    }

    async fn delete_topic(&self, kafka_name: &str) -> Result<(), BusError> {
        let mut topics = self.topics.lock();
        topics
            .remove(kafka_name)
            .ok_or_else(|| BusError::TopicNotFound(kafka_name.into()))?;
        Ok(())
    }

    async fn produce(&self, mut message: BusMessage) -> Result<DeliveryReceipt, BusError> {
        let mut topics = self.topics.lock();
        let state = topics
            .get_mut(&message.topic)
            .ok_or_else(|| BusError::TopicNotFound(message.topic.clone()))?;
        let offset = state.next_offset;
        state.next_offset += 1;
        let timestamp = Utc::now();
        message.partition = Some(0);
        message.offset = Some(offset);
        message.timestamp = Some(timestamp);
        state.log.push_back(message.clone());
        let _ = state.tx.send(message.clone());
        Ok(DeliveryReceipt {
            topic: message.topic,
            partition: 0,
            offset,
            timestamp,
        })
    }

    async fn subscribe(
        &self,
        kafka_topic: &str,
        _group_id: &str,
        from: StartOffset,
    ) -> Result<SubscribeStream, BusError> {
        let (backlog, rx) = {
            let topics = self.topics.lock();
            let state = topics
                .get(kafka_topic)
                .ok_or_else(|| BusError::TopicNotFound(kafka_topic.into()))?;
            let backlog: Vec<BusMessage> = match from {
                StartOffset::Earliest => state.log.iter().cloned().collect(),
                StartOffset::Latest => Vec::new(),
            };
            (backlog, state.tx.subscribe())
        };
        let mut rx = rx;
        let stream = async_stream::stream! {
            for msg in backlog {
                yield Ok(msg);
            }
            while let Ok(msg) = rx.recv().await {
                yield Ok(msg);
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn produce_and_subscribe_earliest() {
        let backend = MemoryBackend::new();
        let topic = Topic::new("t1", "ns", "tn");
        backend.create_topic(&topic).await.unwrap();

        for i in 0..3 {
            backend
                .produce(BusMessage::new(
                    topic.kafka_topic_name(),
                    serde_json::json!({ "n": i }),
                ))
                .await
                .unwrap();
        }

        let mut stream = backend
            .subscribe(
                &topic.kafka_topic_name(),
                "test-group",
                StartOffset::Earliest,
            )
            .await
            .unwrap();
        for i in 0..3 {
            let msg = stream.next().await.unwrap().unwrap();
            assert_eq!(msg.payload["n"], i);
            assert_eq!(msg.offset, Some(i));
        }
    }

    #[tokio::test]
    async fn subscribe_latest_skips_backlog() {
        let backend = MemoryBackend::new();
        let topic = Topic::new("t1", "ns", "tn");
        backend.create_topic(&topic).await.unwrap();

        backend
            .produce(BusMessage::new(
                topic.kafka_topic_name(),
                serde_json::json!({"seen": false}),
            ))
            .await
            .unwrap();

        let mut stream = backend
            .subscribe(&topic.kafka_topic_name(), "g", StartOffset::Latest)
            .await
            .unwrap();
        // Live produce after subscribe — this one should arrive.
        let backend2 = Arc::clone(&backend);
        let topic_name = topic.kafka_topic_name();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            backend2
                .produce(BusMessage::new(
                    topic_name,
                    serde_json::json!({"seen": true}),
                ))
                .await
                .unwrap();
        });
        let msg = stream.next().await.unwrap().unwrap();
        assert_eq!(msg.payload["seen"], true);
    }

    #[tokio::test]
    async fn produce_to_missing_topic_fails() {
        let backend = MemoryBackend::new();
        let err = backend
            .produce(BusMessage::new("ghost", serde_json::json!({})))
            .await
            .unwrap_err();
        assert!(matches!(err, BusError::TopicNotFound(_)));
    }

    #[tokio::test]
    async fn delete_is_idempotent_on_absent() {
        let backend = MemoryBackend::new();
        let err = backend.delete_topic("nope").await.unwrap_err();
        assert!(matches!(err, BusError::TopicNotFound(_)));
    }
}
