//! Finite, receipt-bearing synthetic source for the managed-stage demo.
//! The memory bus backend has no live acknowledgement support; this adapter
//! owns its fixture log and validates exact deliveries before advancing it.
use acteon_bus::{
    BusMessage, StreamPosition, StreamPositionLane, StreamStageRecord, StreamStageSource,
    StreamStageSourceError,
};
use async_trait::async_trait;
use uuid::Uuid;

use super::Transaction;

#[derive(Clone, PartialEq, Eq)]
pub struct FixtureReceipt {
    session: Uuid,
    offset: usize,
}
pub struct TransactionSource {
    identity: String,
    records: Vec<StreamStageRecord<FixtureReceipt>>,
    next: usize,
    closed: bool,
}
impl TransactionSource {
    pub fn new(topic: &str, transactions: &[Transaction]) -> Result<Self, serde_json::Error> {
        let lane = StreamPositionLane {
            source: "bank-source".into(),
            topic: topic.into(),
            consumer_group: "suspicion-filter".into(),
            partition: 0,
        };
        let identity = serde_json::to_string(&lane)?;
        let session = Uuid::new_v4();
        let records = transactions
            .iter()
            .enumerate()
            .map(|(index, tx)| {
                let offset = i64::try_from(index).expect("bounded fixture");
                let mut message =
                    BusMessage::new(topic, serde_json::to_value(tx)?).with_key(&tx.account_id);
                message.partition = Some(0);
                message.offset = Some(offset);
                message.timestamp = Some(tx.occurred_at);
                Ok(StreamStageRecord {
                    position: StreamPosition {
                        lane: lane.clone(),
                        offset,
                    },
                    message,
                    receipt: FixtureReceipt {
                        session,
                        offset: index,
                    },
                })
            })
            .collect::<Result<_, serde_json::Error>>()?;
        Ok(Self {
            identity,
            records,
            next: 0,
            closed: false,
        })
    }
    pub fn acknowledged(&self) -> usize {
        self.next
    }
}
#[async_trait]
impl StreamStageSource for TransactionSource {
    type Receipt = FixtureReceipt;
    fn identity(&self) -> String {
        self.identity.clone()
    }
    async fn receive(
        &mut self,
        max_records: usize,
    ) -> Result<Vec<StreamStageRecord<Self::Receipt>>, StreamStageSourceError> {
        if self.closed {
            return Err(StreamStageSourceError::Fenced(
                "fixture source closed".into(),
            ));
        }
        // Retain the original delivery across cancellation/retry until acknowledged.
        Ok(self
            .records
            .get(self.next)
            .filter(|_| max_records > 0)
            .cloned()
            .into_iter()
            .collect())
    }
    async fn validate(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<Vec<StreamPosition>, StreamStageSourceError> {
        if !self.closed
            && let [record] = records
            && let Some(expected) = self.records.get(self.next)
            && record.receipt == expected.receipt
            && record.position == expected.position
            && record.message == expected.message
        {
            return Ok(vec![expected.position.clone()]);
        }
        Err(StreamStageSourceError::Fenced(
            "receipt does not match the next fixture delivery".into(),
        ))
    }
    async fn acknowledge(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<(), StreamStageSourceError> {
        self.validate(records).await?;
        self.next += 1;
        Ok(())
    }
    async fn recover(&mut self) -> Result<(), StreamStageSourceError> {
        Ok(())
    }
    async fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exact_receipts_are_required_and_unacknowledged_records_are_redelivered() {
        let fixture = super::super::transactions();
        let mut source =
            TransactionSource::new("banking.demo-bank.transactions", &fixture).unwrap();
        let first = source.receive(1).await.unwrap();
        source.recover().await.unwrap();
        assert_eq!(
            source.receive(1).await.unwrap()[0].position,
            first[0].position
        );
        let mut other = TransactionSource::new("banking.demo-bank.transactions", &fixture).unwrap();
        assert!(
            source
                .acknowledge(&other.receive(1).await.unwrap())
                .await
                .is_err()
        );
        source.acknowledge(&first).await.unwrap();
        assert!(source.acknowledge(&first).await.is_err());
        assert_eq!(source.acknowledged(), 1);
        assert_eq!(source.receive(1).await.unwrap()[0].position.offset, 1);
    }
}
