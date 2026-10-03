//! Live consumer acknowledgements with assignment-scoped, opaque receipts.
//!
//! Receipt ownership fences acknowledgements, not external side effects. Poll
//! the subscription within Kafka's configured maximum poll interval. Persist
//! processing state and outputs before acknowledging their receipts.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::{BusError, BusMessage, OffsetPosition};

/// Bounds for records delivered but not yet committed on a live subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// Includes acknowledged records waiting behind an earlier unacknowledged record.
    pub max_in_flight: usize,
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            max_in_flight: 1024,
        }
    }
}

impl SubscriptionConfig {
    pub(crate) fn validate(&self) -> Result<(), SubscriptionError> {
        if self.max_in_flight == 0 || self.max_in_flight > 100_000 {
            return Err(SubscriptionError::InvalidConfig(
                "max_in_flight must be in 1..=100000".into(),
            ));
        }
        Ok(())
    }
}

/// A capability issued for one record by one active consumer assignment.
///
/// Fields are private and receipts cannot be deserialized or reconstructed
/// from raw offsets. Clone to retain evidence across a processing batch.
#[derive(Debug, Clone)]
pub struct SubscriptionReceipt {
    pub(crate) session: uuid::Uuid,
    pub(crate) epoch: u64,
    pub(crate) topic: String,
    pub(crate) consumer_group: String,
    pub(crate) partition: i32,
    pub(crate) offset: i64,
}

impl SubscriptionReceipt {
    /// Broker topic from the original delivery.
    #[must_use]
    pub fn topic(&self) -> &str {
        &self.topic
    }
    /// Consumer group of the live session.
    #[must_use]
    pub fn consumer_group(&self) -> &str {
        &self.consumer_group
    }
    /// Local assignment epoch. Not a broker generation ID.
    #[must_use]
    pub fn assignment_epoch(&self) -> u64 {
        self.epoch
    }
    /// Last consumed offset, not Kafka's next-offset commit representation.
    #[must_use]
    pub fn position(&self) -> OffsetPosition {
        OffsetPosition {
            partition: self.partition,
            offset: self.offset,
        }
    }
}

/// Record and its acknowledgement capability, issued together.
#[derive(Debug, Clone)]
pub struct SubscriptionDelivery {
    pub message: BusMessage,
    pub receipt: SubscriptionReceipt,
}

/// Current ownership. Revocation clears partitions before Kafka unassigns them.
/// Watch notifications coalesce: inspect this current state rather than counting events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionOwnership {
    pub epoch: u64,
    pub partitions: Vec<i32>,
    pub closed: bool,
}

/// Last consumed offsets confirmed by the broker for this acknowledgement.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionAck {
    pub committed: Vec<OffsetPosition>,
    /// Records still in flight, including acknowledgements deferred behind gaps.
    pub remaining_in_flight: usize,
}

/// Failures specific to active consumer ownership and acknowledgement.
#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error(transparent)]
    Bus(#[from] BusError),
    #[error("invalid subscription configuration: {0}")]
    InvalidConfig(String),
    #[error("backend does not support live acknowledgements")]
    Unsupported,
    #[error("receipt belongs to another subscription")]
    WrongSubscription,
    #[error("receipt assignment has been revoked or lost")]
    StaleReceipt,
    #[error("receipt was not delivered by this assignment")]
    UnknownReceipt,
    #[error("checkpoint receipts do not cover the complete commit prefix")]
    CheckpointGap,
    #[error("subscription has {limit} records in flight; acknowledge before receiving more")]
    Capacity { limit: usize },
    #[error("subscription is closed")]
    Closed,
    #[error("invalid broker record: {0}")]
    InvalidRecord(String),
}

/// A long-lived consumer. Acknowledgements use the consumer that delivered
/// records, never a replacement member joining solely to commit raw offsets.
#[async_trait]
pub trait AcknowledgedSubscription: Send {
    /// Poll one record. Cancellation before delivery does not acknowledge it.
    /// On capacity, finish the existing batch before receiving more records.
    async fn recv(&mut self) -> Result<SubscriptionDelivery, SubscriptionError>;
    /// Validate session and current assignment without committing offsets.
    fn validate_receipts(&self, receipts: &[SubscriptionReceipt]) -> Result<(), SubscriptionError>;
    /// Also require the complete commit prefix on each selected
    /// partition. A checkpoint's high-water mark must not skip processing gaps.
    fn validate_checkpoint_receipts(
        &self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<(), SubscriptionError>;
    /// Mark receipts processed and commit each partition's processed prefix.
    /// A later receipt cannot commit past an earlier unprocessed delivery.
    /// Failed commits retain the prefix for a retry using the same receipts.
    async fn acknowledge(
        &mut self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<SubscriptionAck, SubscriptionError>;
    /// Current local ownership, updated when rebalance callbacks are polled.
    fn ownership_changes(&self) -> watch::Receiver<SubscriptionOwnership>;
}

#[derive(Default)]
struct PartitionReceipts {
    pending: BTreeMap<i64, bool>,
    last_delivered: Option<i64>,
    committed: Option<i64>,
}

/// Shared by the consumer context and serialized subscription operations. Never
/// hold this lock across a librdkafka call: a call may run rebalance callbacks.
pub(crate) struct ReceiptLedger {
    session: uuid::Uuid,
    topic: String,
    group: String,
    max_in_flight: usize,
    epoch: u64,
    closed: bool,
    partitions: BTreeMap<i32, PartitionReceipts>,
    ownership: watch::Sender<SubscriptionOwnership>,
}

impl ReceiptLedger {
    pub(crate) fn new(topic: String, group: String, config: &SubscriptionConfig) -> Self {
        let (ownership, _) = watch::channel(SubscriptionOwnership::default());
        Self {
            session: uuid::Uuid::new_v4(),
            topic,
            group,
            max_in_flight: config.max_in_flight,
            epoch: 0,
            closed: false,
            partitions: BTreeMap::new(),
            ownership,
        }
    }
    pub(crate) fn watch(&self) -> watch::Receiver<SubscriptionOwnership> {
        self.ownership.subscribe()
    }
    pub(crate) fn transition(&mut self, partitions: impl IntoIterator<Item = i32>, closed: bool) {
        // Never reuse an epoch, including the same partition being reassigned.
        match self.epoch.checked_add(1) {
            Some(epoch) => self.epoch = epoch,
            None => self.closed = true,
        }
        self.closed |= closed;
        self.partitions.clear();
        if !self.closed {
            self.partitions.extend(
                partitions
                    .into_iter()
                    .map(|p| (p, PartitionReceipts::default())),
            );
        }
        self.ownership.send_replace(SubscriptionOwnership {
            epoch: self.epoch,
            partitions: self.partitions.keys().copied().collect(),
            closed: self.closed,
        });
    }
    fn count(&self) -> usize {
        self.partitions.values().map(|p| p.pending.len()).sum()
    }
    pub(crate) fn check_capacity(&self) -> Result<(), SubscriptionError> {
        if self.closed {
            return Err(SubscriptionError::Closed);
        }
        if self.count() >= self.max_in_flight {
            return Err(SubscriptionError::Capacity {
                limit: self.max_in_flight,
            });
        }
        Ok(())
    }
    pub(crate) fn deliver(
        &mut self,
        partition: i32,
        offset: i64,
    ) -> Result<SubscriptionReceipt, SubscriptionError> {
        self.check_capacity()?;
        if offset < 0 || offset == i64::MAX {
            return Err(SubscriptionError::InvalidRecord(
                "offset cannot be committed".into(),
            ));
        }
        let lane = self
            .partitions
            .get_mut(&partition)
            .ok_or(SubscriptionError::StaleReceipt)?;
        if lane.last_delivered.is_some_and(|last| offset <= last) {
            return Err(SubscriptionError::InvalidRecord(
                "partition delivery regressed".into(),
            ));
        }
        lane.last_delivered = Some(offset);
        lane.pending.insert(offset, false);
        Ok(SubscriptionReceipt {
            session: self.session,
            epoch: self.epoch,
            topic: self.topic.clone(),
            consumer_group: self.group.clone(),
            partition,
            offset,
        })
    }
    pub(crate) fn validate(
        &self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<(), SubscriptionError> {
        if self.closed {
            return Err(SubscriptionError::Closed);
        }
        for receipt in receipts {
            if receipt.session != self.session
                || receipt.topic != self.topic
                || receipt.consumer_group != self.group
            {
                return Err(SubscriptionError::WrongSubscription);
            }
            if receipt.epoch != self.epoch {
                return Err(SubscriptionError::StaleReceipt);
            }
            let lane = self
                .partitions
                .get(&receipt.partition)
                .ok_or(SubscriptionError::StaleReceipt)?;
            if !lane.pending.contains_key(&receipt.offset)
                && lane.committed.is_none_or(|offset| receipt.offset > offset)
            {
                return Err(SubscriptionError::UnknownReceipt);
            }
        }
        Ok(())
    }
    pub(crate) fn validate_checkpoint(
        &self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<(), SubscriptionError> {
        self.validate(receipts)?;
        let included = receipts
            .iter()
            .map(|r| (r.partition, r.offset))
            .collect::<BTreeSet<_>>();
        let mut maxima = BTreeMap::new();
        for receipt in receipts {
            maxima
                .entry(receipt.partition)
                .and_modify(|offset: &mut i64| *offset = (*offset).max(receipt.offset))
                .or_insert(receipt.offset);
        }
        for (partition, maximum) in maxima {
            let lane = &self.partitions[&partition];
            if lane
                .pending
                .range(..=maximum)
                .any(|(offset, _)| !included.contains(&(partition, *offset)))
            {
                return Err(SubscriptionError::CheckpointGap);
            }
            if lane
                .pending
                .range((
                    std::ops::Bound::Excluded(maximum),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                .is_some_and(|(_, processed)| *processed)
            {
                // A deferred acknowledgement must not extend the commit beyond
                // the source position represented by this checkpoint.
                return Err(SubscriptionError::CheckpointGap);
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_ack(
        &mut self,
        receipts: &[SubscriptionReceipt],
    ) -> Result<(u64, Vec<OffsetPosition>), SubscriptionError> {
        // Validate the whole batch before modifying any acknowledgement state.
        self.validate(receipts)?;
        let touched = receipts
            .iter()
            .filter(|receipt| {
                self.partitions[&receipt.partition]
                    .pending
                    .contains_key(&receipt.offset)
            })
            .map(|r| r.partition)
            .collect::<BTreeSet<_>>();
        for receipt in receipts {
            if let Some(processed) = self
                .partitions
                .get_mut(&receipt.partition)
                .and_then(|p| p.pending.get_mut(&receipt.offset))
            {
                *processed = true;
            }
        }
        let commits = touched
            .into_iter()
            .filter_map(|partition| {
                let lane = &self.partitions[&partition];
                lane.pending
                    .iter()
                    .take_while(|(_, processed)| **processed)
                    .last()
                    .map(|(offset, _)| OffsetPosition {
                        partition,
                        offset: *offset,
                    })
            })
            .collect();
        Ok((self.epoch, commits))
    }
    pub(crate) fn finish_ack(
        &mut self,
        epoch: u64,
        committed: Vec<OffsetPosition>,
    ) -> Result<SubscriptionAck, SubscriptionError> {
        if self.closed {
            return Err(SubscriptionError::Closed);
        }
        if epoch != self.epoch {
            return Err(SubscriptionError::StaleReceipt);
        }
        for position in &committed {
            let lane = self
                .partitions
                .get_mut(&position.partition)
                .ok_or(SubscriptionError::StaleReceipt)?;
            lane.pending.retain(|offset, _| *offset > position.offset);
            lane.committed = Some(position.offset);
        }
        Ok(SubscriptionAck {
            committed,
            remaining_in_flight: self.count(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ledger(limit: usize) -> ReceiptLedger {
        let mut ledger = ReceiptLedger::new(
            "topic".into(),
            "group".into(),
            &SubscriptionConfig {
                max_in_flight: limit,
            },
        );
        ledger.transition([0, 1], false);
        ledger
    }
    #[test]
    fn acknowledgements_cannot_skip_a_gap_and_broker_offset_gaps_are_allowed() {
        let mut ledger = ledger(10);
        let first = ledger.deliver(0, 4).unwrap();
        let later = ledger.deliver(0, 9).unwrap(); // compacted/transactional log gap
        let (epoch, commits) = ledger.prepare_ack(&[later]).unwrap();
        assert!(commits.is_empty());
        assert_eq!(
            ledger
                .finish_ack(epoch, commits)
                .unwrap()
                .remaining_in_flight,
            2
        );
        let (epoch, commits) = ledger.prepare_ack(&[first.clone()]).unwrap();
        assert_eq!(commits[0].offset, 9);
        assert_eq!(
            ledger
                .finish_ack(epoch, commits)
                .unwrap()
                .remaining_in_flight,
            0
        );
        assert!(ledger.prepare_ack(&[first]).unwrap().1.is_empty());
    }
    #[test]
    fn revoke_and_reassign_the_same_partition_invalidates_every_old_receipt() {
        let mut ledger = ledger(10);
        let old = ledger.deliver(0, 0).unwrap();
        let mut changes = ledger.watch();
        ledger.transition([], false);
        assert!(changes.has_changed().unwrap());
        assert!(changes.borrow_and_update().partitions.is_empty());
        ledger.transition([0], false);
        assert!(matches!(
            ledger.validate(&[old]),
            Err(SubscriptionError::StaleReceipt)
        ));
        let new = ledger.deliver(0, 0).unwrap();
        assert_eq!(new.assignment_epoch(), 3);
    }
    #[test]
    fn cross_session_batch_is_rejected_atomically() {
        let mut first = ledger(10);
        let mut second = ledger(10);
        let own = first.deliver(0, 0).unwrap();
        let other = second.deliver(0, 0).unwrap();
        assert!(matches!(
            first.prepare_ack(&[own, other]),
            Err(SubscriptionError::WrongSubscription)
        ));
        assert!(!first.partitions[&0].pending[&0]);
    }
    #[test]
    fn failed_commit_is_retryable_but_revocation_fences_its_completion() {
        let mut ledger = ledger(10);
        let receipt = ledger.deliver(0, 0).unwrap();
        let (_, first) = ledger.prepare_ack(&[receipt.clone()]).unwrap();
        let (epoch, retry) = ledger.prepare_ack(&[receipt]).unwrap();
        assert_eq!(first[0].offset, retry[0].offset);
        ledger.transition([], false);
        assert!(matches!(
            ledger.finish_ack(epoch, retry),
            Err(SubscriptionError::StaleReceipt)
        ));
    }
    #[test]
    fn capacity_remains_bounded_until_a_processed_prefix_is_committed() {
        let mut ledger = ledger(2);
        let first = ledger.deliver(0, 0).unwrap();
        let later = ledger.deliver(0, 1).unwrap();
        assert!(matches!(
            ledger.deliver(1, 0),
            Err(SubscriptionError::Capacity { limit: 2 })
        ));
        ledger.prepare_ack(&[later]).unwrap();
        assert!(ledger.check_capacity().is_err());
        let (epoch, commits) = ledger.prepare_ack(&[first]).unwrap();
        ledger.finish_ack(epoch, commits).unwrap();
        ledger.deliver(1, 0).unwrap();
    }
    #[test]
    fn partitions_advance_independently_and_closed_sessions_cannot_ack() {
        let mut ledger = ledger(10);
        ledger.deliver(0, 0).unwrap();
        let other = ledger.deliver(1, 7).unwrap();
        let (epoch, commits) = ledger.prepare_ack(&[other.clone()]).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].partition, 1);
        ledger.finish_ack(epoch, commits).unwrap();
        ledger.transition([], true);
        assert!(matches!(
            ledger.validate(&[other]),
            Err(SubscriptionError::Closed)
        ));
    }

    #[test]
    fn a_checkpoint_must_cover_deferred_acknowledgements_that_extend_its_commit() {
        let mut ledger = ledger(10);
        let first = ledger.deliver(0, 0).unwrap();
        let later = ledger.deliver(0, 1).unwrap();
        assert!(ledger.prepare_ack(&[later.clone()]).unwrap().1.is_empty());
        assert!(matches!(
            ledger.validate_checkpoint(&[first.clone()]),
            Err(SubscriptionError::CheckpointGap)
        ));
        assert!(ledger.validate_checkpoint(&[first, later]).is_ok());
    }

    #[tokio::test]
    async fn unsupported_backends_do_not_fall_back_to_raw_commits() {
        use crate::BusBackend;
        assert!(matches!(
            crate::MemoryBackend::new()
                .subscribe_acknowledged(
                    "topic",
                    "group",
                    crate::StartOffset::Earliest,
                    SubscriptionConfig::default()
                )
                .await,
            Err(SubscriptionError::Unsupported)
        ));
    }
}
