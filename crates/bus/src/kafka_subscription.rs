//! Receipt-based acknowledgements on the original Kafka consumer.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use parking_lot::Mutex;
use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{
    BaseConsumer, CommitMode, Consumer, ConsumerContext, Rebalance, StreamConsumer,
};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::{Headers, Message};
use rdkafka::{Offset, TopicPartitionList};
use tokio::sync::watch;

use crate::subscription::ReceiptLedger;
use crate::{
    AcknowledgedSubscription, BusError, BusMessage, SubscriptionAck, SubscriptionConfig,
    SubscriptionDelivery, SubscriptionError, SubscriptionOwnership, SubscriptionReceipt,
};

struct ReceiptContext {
    ledger: Arc<Mutex<ReceiptLedger>>,
}
impl ClientContext for ReceiptContext {}
impl ConsumerContext for ReceiptContext {
    fn pre_rebalance(&self, _: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        if !matches!(rebalance, Rebalance::Assign(_)) {
            self.ledger.lock().transition([], false);
        }
    }
    fn post_rebalance(&self, _: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        if let Rebalance::Assign(partitions) = rebalance {
            self.ledger.lock().transition(
                partitions
                    .elements()
                    .iter()
                    .map(rdkafka::topic_partition_list::TopicPartitionListElem::partition),
                false,
            );
        }
    }
}

struct KafkaSubscription {
    consumer: Arc<StreamConsumer<ReceiptContext>>,
    ledger: Arc<Mutex<ReceiptLedger>>,
    // A cancelled acknowledgement must not release serialization while its
    // synchronous broker operation still runs on a blocking worker.
    operation: Arc<tokio::sync::Mutex<()>>,
}

pub(crate) fn open(
    mut config: ClientConfig,
    topic: &str,
    group: &str,
    bounds: &SubscriptionConfig,
) -> Result<Box<dyn AcknowledgedSubscription>, SubscriptionError> {
    bounds.validate()?;
    if topic.trim().is_empty() || topic.starts_with('^') || group.trim().is_empty() {
        return Err(SubscriptionError::InvalidConfig(
            "a nonempty literal topic and group are required; regex topics are unsupported".into(),
        ));
    }
    // These correctness properties cannot be overridden by pass-through config.
    config.set("enable.auto.commit", "false");
    config.set("enable.auto.offset.store", "false");
    config.set("group.id", group);
    config.set("group.protocol", "classic");
    config.set("enable.partition.eof", "false");
    // A full revoke invalidates the entire local epoch. Cooperative assignment
    // needs a different per-partition epoch protocol and is not silently enabled.
    config.set("partition.assignment.strategy", "range");
    let ledger = Arc::new(Mutex::new(ReceiptLedger::new(
        topic.into(),
        group.into(),
        bounds,
    )));
    let consumer: StreamConsumer<ReceiptContext> = config
        .create_with_context(ReceiptContext {
            ledger: Arc::clone(&ledger),
        })
        .map_err(transport)?;
    consumer.subscribe(&[topic]).map_err(transport)?;
    Ok(Box::new(KafkaSubscription {
        consumer: Arc::new(consumer),
        ledger,
        operation: Arc::new(tokio::sync::Mutex::new(())),
    }))
}

fn transport(error: impl std::fmt::Display) -> SubscriptionError {
    SubscriptionError::Bus(BusError::Transport(error.to_string()))
}

fn invalidate_lost(
    consumer: &StreamConsumer<ReceiptContext>,
    ledger: &Mutex<ReceiptLedger>,
) -> Result<(), SubscriptionError> {
    if consumer.assignment_lost() {
        ledger.lock().transition([], false);
        return Err(SubscriptionError::StaleReceipt);
    }
    Ok(())
}

fn decode(message: &impl Message) -> Result<BusMessage, SubscriptionError> {
    let payload = message
        .payload()
        .map_or(Ok(serde_json::Value::Null), serde_json::from_slice)
        .map_err(|error| SubscriptionError::InvalidRecord(format!("invalid JSON: {error}")))?;
    let mut headers = std::collections::BTreeMap::new();
    if let Some(source) = message.headers() {
        for index in 0..source.count() {
            let header = source.get(index);
            if let Some(value) = header
                .value
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
            {
                headers.insert(header.key.to_owned(), value.to_owned());
            }
        }
    }
    Ok(BusMessage {
        topic: message.topic().to_owned(),
        key: message
            .key()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map(str::to_owned),
        payload,
        headers,
        partition: Some(message.partition()),
        offset: Some(message.offset()),
        timestamp: message
            .timestamp()
            .to_millis()
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
    })
}

#[async_trait]
impl AcknowledgedSubscription for KafkaSubscription {
    async fn recv(&mut self) -> Result<SubscriptionDelivery, SubscriptionError> {
        let _operation = self.operation.lock().await;
        self.ledger.lock().check_capacity()?;
        loop {
            let message = match self.consumer.recv().await {
                Ok(message) => message,
                Err(error) if crate::kafka::is_recoverable_consumer_error(&error) => {
                    tracing::warn!(%error, "Kafka acknowledgement subscription reconnecting");
                    continue;
                }
                Err(error) => {
                    self.ledger.lock().transition([], true);
                    return Err(transport(error));
                }
            };
            let delivery = decode(&message).and_then(|decoded| {
                invalidate_lost(&self.consumer, &self.ledger)?;
                let receipt = self
                    .ledger
                    .lock()
                    .deliver(message.partition(), message.offset())?;
                Ok(SubscriptionDelivery {
                    message: decoded,
                    receipt,
                })
            });
            if delivery.is_err() {
                // A decoded/polled record cannot be silently skipped: no future
                // acknowledgement may commit past an undispatched malformed record.
                self.ledger.lock().transition([], true);
            }
            return delivery;
        }
    }

    fn validate_receipts(&self, receipts: &[SubscriptionReceipt]) -> Result<(), SubscriptionError> {
        invalidate_lost(&self.consumer, &self.ledger)?;
        self.ledger.lock().validate(receipts)
    }

    fn validate_checkpoint_receipts(
        &self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<(), SubscriptionError> {
        invalidate_lost(&self.consumer, &self.ledger)?;
        self.ledger.lock().validate_checkpoint(receipts)
    }

    async fn acknowledge(
        &mut self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<SubscriptionAck, SubscriptionError> {
        let operation = Arc::clone(&self.operation).lock_owned().await;
        let consumer = Arc::clone(&self.consumer);
        let ledger = Arc::clone(&self.ledger);
        let receipts = receipts.to_vec();
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            invalidate_lost(&consumer, &ledger)?;
            let (epoch, commits) = ledger.lock().prepare_ack(&receipts)?;
            if !commits.is_empty() {
                let mut positions = TopicPartitionList::new();
                for position in &commits {
                    positions
                        .add_partition_offset(
                            &receipts[0].topic,
                            position.partition,
                            Offset::Offset(position.offset + 1),
                        )
                        .map_err(transport)?;
                }
                // Keep the ledger unlocked while committing: callbacks can revoke
                // ownership. The broker also checks this live member's generation.
                if let Err(error) = consumer.commit(&positions, CommitMode::Sync) {
                    if matches!(
                        error,
                        KafkaError::ConsumerCommit(
                            RDKafkaErrorCode::IllegalGeneration
                                | RDKafkaErrorCode::UnknownMemberId
                                | RDKafkaErrorCode::RebalanceInProgress
                                | RDKafkaErrorCode::FencedInstanceId
                                | RDKafkaErrorCode::FencedMemberEpoch
                                | RDKafkaErrorCode::StaleMemberEpoch
                        )
                    ) {
                        ledger.lock().transition([], false);
                        return Err(SubscriptionError::StaleReceipt);
                    }
                    invalidate_lost(&consumer, &ledger)?;
                    ledger.lock().validate(&receipts)?;
                    return Err(transport(error));
                }
            }
            invalidate_lost(&consumer, &ledger)?;
            ledger.lock().finish_ack(epoch, commits)
        })
        .await
        .map_err(transport)?
    }

    fn ownership_changes(&self) -> watch::Receiver<SubscriptionOwnership> {
        self.ledger.lock().watch()
    }
}

impl Drop for KafkaSubscription {
    fn drop(&mut self) {
        self.ledger.lock().transition([], true);
        // A blocking acknowledgement still owns its Arc and operation guard.
        // Closing the ledger fences its completion; Kafka also fences lost membership.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_subscriptions_cannot_issue_receipts_for_a_literal_topic() {
        assert!(matches!(
            open(
                ClientConfig::new(),
                "^observability.*",
                "group",
                &SubscriptionConfig::default()
            ),
            Err(SubscriptionError::InvalidConfig(_))
        ));
    }
}
