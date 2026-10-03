//! Receipt-bearing transport adapters for managed processing stages.
use crate::{
    AcknowledgedSubscription, BusMessage, SharedBackend, StartOffset, StreamPosition,
    StreamPositionLane, SubscriptionConfig, SubscriptionError, SubscriptionReceipt,
};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StreamStageSourceError {
    #[error("retryable source failure: {0}")]
    Retryable(String),
    #[error("source ownership lost: {0}")]
    Fenced(String),
    #[error("permanent source failure: {0}")]
    Permanent(String),
}
impl From<SubscriptionError> for StreamStageSourceError {
    fn from(e: SubscriptionError) -> Self {
        match e {
            SubscriptionError::StaleReceipt
            | SubscriptionError::Closed
            | SubscriptionError::WrongSubscription
            | SubscriptionError::UnknownReceipt => Self::Fenced(e.to_string()),
            SubscriptionError::Bus(_) => Self::Retryable(e.to_string()),
            _ => Self::Permanent(e.to_string()),
        }
    }
}
/// The receipt remains opaque; only the adapter can validate and acknowledge it.
#[derive(Debug, Clone)]
pub struct StreamStageRecord<R> {
    pub position: StreamPosition,
    pub message: BusMessage,
    pub receipt: R,
}
/// Sources retain delivered capabilities on receive cancellation and retry.
/// Validation MUST require the complete processed prefix. Acknowledgement MUST
/// use the original consumer and revalidate ownership; raw-offset adapters are unsafe.
#[async_trait]
pub trait StreamStageSource: Send {
    type Receipt: Clone + Send + Sync;
    fn identity(&self) -> String;
    async fn receive(
        &mut self,
        max_records: usize,
    ) -> Result<Vec<StreamStageRecord<Self::Receipt>>, StreamStageSourceError>;
    async fn validate(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<Vec<StreamPosition>, StreamStageSourceError>;
    async fn acknowledge(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<(), StreamStageSourceError>;
    async fn recover(&mut self) -> Result<(), StreamStageSourceError>;
    /// Release consumers without committing; the next receive may reconnect.
    async fn close(&mut self);
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStageSubscription {
    pub source: String,
    pub topic: String,
    pub consumer_group: String,
    pub starting_offset: StartOffset,
}
impl StreamStageSubscription {
    pub fn new(
        source: impl Into<String>,
        topic: impl Into<String>,
        consumer_group: impl Into<String>,
        starting_offset: StartOffset,
    ) -> Self {
        Self {
            source: source.into(),
            topic: topic.into(),
            consumer_group: consumer_group.into(),
            starting_offset,
        }
    }
}
#[derive(Debug, Clone)]
pub struct LiveStageReceipt {
    source: usize,
    receipt: SubscriptionReceipt,
}
pub struct LiveStreamStageSource {
    backend: SharedBackend,
    definitions: Vec<StreamStageSubscription>,
    config: SubscriptionConfig,
    subscriptions: Vec<Box<dyn AcknowledgedSubscription>>,
    pending: Vec<StreamStageRecord<LiveStageReceipt>>,
}
impl LiveStreamStageSource {
    pub fn connect(
        backend: SharedBackend,
        mut definitions: Vec<StreamStageSubscription>,
        config: SubscriptionConfig,
    ) -> Result<Self, StreamStageSourceError> {
        definitions.sort_by(|a, b| a.source.cmp(&b.source));
        let mut names = BTreeSet::new();
        let mut groups = BTreeSet::new();
        if definitions.is_empty()
            || definitions.iter().any(|d| {
                d.source.trim().is_empty()
                    || d.topic.trim().is_empty()
                    || d.consumer_group.trim().is_empty()
                    || !names.insert(d.source.clone())
                    || !groups.insert((d.topic.clone(), d.consumer_group.clone()))
            })
        {
            return Err(StreamStageSourceError::Permanent(
                "source definitions must be nonempty and uniquely named/topic-group bound".into(),
            ));
        }
        let source = Self {
            backend,
            definitions,
            config,
            subscriptions: vec![],
            pending: vec![],
        };
        source.config.validate()?;
        Ok(source)
    }
    async fn open(&mut self) -> Result<(), StreamStageSourceError> {
        // Resume partially opened sources after receive cancellation.
        for d in &self.definitions[self.subscriptions.len()..] {
            self.subscriptions.push(
                self.backend
                    .subscribe_acknowledged(
                        &d.topic,
                        &d.consumer_group,
                        d.starting_offset,
                        self.config.clone(),
                    )
                    .await?,
            );
        }
        Ok(())
    }
    fn grouped(
        &self,
        records: &[StreamStageRecord<LiveStageReceipt>],
    ) -> Result<BTreeMap<usize, Vec<SubscriptionReceipt>>, StreamStageSourceError> {
        let mut batches: BTreeMap<usize, Vec<SubscriptionReceipt>> = BTreeMap::new();
        for r in records {
            let i = r.receipt.source;
            let d = self
                .definitions
                .get(i)
                .ok_or_else(|| StreamStageSourceError::Fenced("unknown source".into()))?;
            let receipt = &r.receipt.receipt;
            if r.position.lane.source != d.source
                || r.position.lane.topic != d.topic
                || r.position.lane.consumer_group != d.consumer_group
                || r.position.offset != receipt.position().offset
                || r.position.lane.partition != receipt.position().partition
                || receipt.topic() != d.topic
                || receipt.consumer_group() != d.consumer_group
            {
                return Err(StreamStageSourceError::Fenced(
                    "receipt identity does not match source position".into(),
                ));
            }
            batches.entry(i).or_default().push(receipt.clone());
        }
        Ok(batches)
    }
}
#[async_trait]
impl StreamStageSource for LiveStreamStageSource {
    type Receipt = LiveStageReceipt;
    fn identity(&self) -> String {
        serde_json::to_string(&self.definitions).expect("definitions serialize")
    }
    async fn receive(
        &mut self,
        max_records: usize,
    ) -> Result<Vec<StreamStageRecord<Self::Receipt>>, StreamStageSourceError> {
        if max_records == 0 || max_records > self.config.max_in_flight {
            return Err(StreamStageSourceError::Permanent(
                "invalid source batch size".into(),
            ));
        }
        if self.subscriptions.len() < self.definitions.len() {
            self.open().await?;
        }
        // Drop revoked capabilities before replaying the cache.
        self.pending.retain(|r| {
            r.receipt.receipt.assignment_epoch()
                == self.subscriptions[r.receipt.source]
                    .ownership_changes()
                    .borrow()
                    .epoch
        });
        if self.pending.is_empty() {
            while self.pending.len() < max_records {
                let polls = self
                    .subscriptions
                    .iter_mut()
                    .enumerate()
                    .map(|(i, s)| Box::pin(async move { (i, s.recv().await) }))
                    .collect::<Vec<_>>();
                let next = if self.pending.is_empty() {
                    Some(futures::future::select_all(polls).await.0)
                } else {
                    tokio::time::timeout(
                        Duration::from_millis(10),
                        futures::future::select_all(polls),
                    )
                    .await
                    .ok()
                    .map(|r| r.0)
                };
                let Some((i, result)) = next else { break };
                let delivery = result?;
                let p = delivery.receipt.position();
                let d = &self.definitions[i];
                self.pending.push(StreamStageRecord {
                    position: StreamPosition {
                        lane: StreamPositionLane {
                            source: d.source.clone(),
                            topic: d.topic.clone(),
                            consumer_group: d.consumer_group.clone(),
                            partition: p.partition,
                        },
                        offset: p.offset,
                    },
                    message: delivery.message,
                    receipt: LiveStageReceipt {
                        source: i,
                        receipt: delivery.receipt,
                    },
                });
            }
        }
        Ok(self.pending.iter().take(max_records).cloned().collect())
    }
    async fn validate(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<Vec<StreamPosition>, StreamStageSourceError> {
        for (i, receipts) in self.grouped(records)? {
            self.subscriptions[i].validate_checkpoint_receipts(&receipts)?;
        }
        Ok(records.iter().map(|r| r.position.clone()).collect())
    }
    async fn acknowledge(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<(), StreamStageSourceError> {
        let batches = self.grouped(records)?;
        for (i, receipts) in &batches {
            self.subscriptions[*i].validate_checkpoint_receipts(receipts)?;
        }
        for (i, receipts) in batches {
            let ack = self.subscriptions[i].acknowledge(&receipts).await?;
            self.pending.retain(|r| {
                r.receipt.source != i
                    || !ack.committed.iter().any(|p| {
                        p.partition == r.position.lane.partition && p.offset >= r.position.offset
                    })
            });
        }
        Ok(())
    }
    async fn recover(&mut self) -> Result<(), StreamStageSourceError> {
        self.close().await;
        Ok(())
    }
    async fn close(&mut self) {
        let old = std::mem::take(&mut self.subscriptions);
        self.pending.clear();
        let _ = tokio::task::spawn_blocking(move || drop(old)).await;
    }
}
