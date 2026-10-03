//! HTTP receipt sessions as a source for Acteon's managed stream stages.
use crate::{ActeonClient, BusSubscription, Error, ReceiveRequest};
use acteon_bus::{
    BusMessage, StreamPosition, StreamPositionLane, StreamStageRecord, StreamStageSource,
    StreamStageSourceError,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpStageSubscription {
    source: String,
    namespace: String,
    tenant: String,
    id: String,
    topic: String,
    consumer_group: String,
}
impl HttpStageSubscription {
    pub fn new(
        source: impl Into<String>,
        subscription: &BusSubscription,
    ) -> Result<Self, StreamStageSourceError> {
        let source = source.into();
        if source.trim().is_empty()
            || !subscription.receipt_required
            || subscription.ack_mode != "manual"
            || subscription.consumer_group.is_empty()
        {
            return Err(StreamStageSourceError::Permanent(
                "a named manual receipt-required subscription is required".into(),
            ));
        }
        Ok(Self {
            source,
            namespace: subscription.namespace.clone(),
            tenant: subscription.tenant.clone(),
            id: subscription.id.clone(),
            topic: subscription.topic.clone(),
            consumer_group: subscription.consumer_group.clone(),
        })
    }
}
#[derive(Debug, Clone)]
pub struct HttpStageReceipt {
    source: usize,
    session: String,
    receipt_id: String,
}
pub struct HttpStreamStageSource {
    client: Arc<ActeonClient>,
    definitions: Vec<HttpStageSubscription>,
    sessions: Vec<String>,
    open_request_ids: Vec<String>,
}
impl HttpStreamStageSource {
    pub fn connect(
        client: Arc<ActeonClient>,
        mut definitions: Vec<HttpStageSubscription>,
    ) -> Result<Self, StreamStageSourceError> {
        definitions.sort_by(|a, b| a.source.cmp(&b.source));
        let mut names = BTreeSet::new();
        let mut groups = BTreeSet::new();
        if definitions.is_empty()
            || definitions.iter().any(|d| {
                !names.insert(d.source.clone())
                    || !groups.insert((d.topic.clone(), d.consumer_group.clone()))
            })
        {
            return Err(StreamStageSourceError::Permanent(
                "sources must be nonempty and uniquely named/topic-group bound".into(),
            ));
        }
        let source = Self {
            client,
            open_request_ids: definitions
                .iter()
                .map(|_| uuid::Uuid::new_v4().to_string())
                .collect(),
            definitions,
            sessions: vec![],
        };
        Ok(source)
    }
    async fn open(&mut self) -> Result<(), StreamStageSourceError> {
        for i in self.sessions.len()..self.definitions.len() {
            let d = &self.definitions[i];
            // Retain the request ID across cancellation and ambiguous responses.
            let mut last = None;
            for _ in 0..3 {
                match self
                    .client
                    .open_bus_session(&d.namespace, &d.tenant, &d.id, &self.open_request_ids[i])
                    .await
                {
                    Ok(s) => {
                        if s.consumer_group != d.consumer_group {
                            let _ = self
                                .client
                                .close_bus_session(&d.namespace, &d.tenant, &d.id, &s.session_id)
                                .await;
                            return Err(StreamStageSourceError::Permanent(
                                "consumer group changed".into(),
                            ));
                        }
                        self.sessions.push(s.session_id);
                        last = None;
                        break;
                    }
                    Err(Error::Http { status: 410, .. }) => {
                        self.open_request_ids[i] = uuid::Uuid::new_v4().to_string();
                        last = Some(StreamStageSourceError::Fenced(
                            "expired HTTP open request".into(),
                        ));
                    }
                    Err(e) => {
                        let retry = e.is_retryable();
                        last = Some(map_error(&e));
                        if !retry {
                            break;
                        }
                    }
                }
            }
            if let Some(e) = last {
                return Err(e);
            }
        }
        Ok(())
    }
    /// Current server-local session IDs, for diagnostics only.
    pub fn session_ids(&self) -> &[String] {
        &self.sessions
    }
    /// Close capabilities without acknowledging pending work. On process death,
    /// server-side receipt/idle/lifetime limits reclaim sessions.
    pub async fn close(&mut self) {
        while let Some(s) = self.sessions.last().cloned() {
            let i = self.sessions.len() - 1;
            let d = &self.definitions[i];
            let _ = self
                .client
                .close_bus_session(&d.namespace, &d.tenant, &d.id, &s)
                .await;
            self.sessions.pop();
            self.open_request_ids[i] = uuid::Uuid::new_v4().to_string();
        }
    }
    fn grouped(
        &self,
        records: &[StreamStageRecord<HttpStageReceipt>],
    ) -> Result<BTreeMap<usize, Vec<String>>, StreamStageSourceError> {
        let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for r in records {
            let i = r.receipt.source;
            let d = self
                .definitions
                .get(i)
                .ok_or_else(|| StreamStageSourceError::Fenced("unknown HTTP source".into()))?;
            if self.sessions.get(i) != Some(&r.receipt.session)
                || r.position.lane.source != d.source
                || r.position.lane.topic != d.topic
                || r.position.lane.consumer_group != d.consumer_group
            {
                return Err(StreamStageSourceError::Fenced(
                    "receipt belongs to another HTTP source or session".into(),
                ));
            }
            groups
                .entry(i)
                .or_default()
                .push(r.receipt.receipt_id.clone());
        }
        Ok(groups)
    }
}
#[async_trait]
impl StreamStageSource for HttpStreamStageSource {
    type Receipt = HttpStageReceipt;
    fn identity(&self) -> String {
        serde_json::to_string(&self.definitions).expect("HTTP definitions serialize")
    }
    async fn receive(
        &mut self,
        max_records: usize,
    ) -> Result<Vec<StreamStageRecord<Self::Receipt>>, StreamStageSourceError> {
        if self.sessions.len() < self.definitions.len() {
            self.open().await?;
        }
        if self.sessions.len() != self.definitions.len() {
            return Err(StreamStageSourceError::Fenced(
                "HTTP source needs recovery".into(),
            ));
        }
        if max_records < self.definitions.len() {
            return Err(StreamStageSourceError::Permanent(
                "batch must allow at least one record per HTTP source".into(),
            ));
        }
        let client = &self.client;
        let sessions = &self.sessions;
        let count = self.definitions.len();
        let requests = self.definitions.iter().enumerate().map(|(i, d)| {
            let max_messages = max_records / count + usize::from(i < max_records % count);
            let session = &sessions[i];
            async move {
                let response = client
                    .receive_bus_session(
                        &d.namespace,
                        &d.tenant,
                        &d.id,
                        session,
                        &ReceiveRequest {
                            max_messages,
                            wait_ms: 1000,
                        },
                    )
                    .await
                    .map_err(|e| map_error(&e))?;
                response
                    .deliveries
                    .into_iter()
                    .map(|delivery| {
                        let message: BusMessage = serde_json::from_value(delivery.message)
                            .map_err(|e| StreamStageSourceError::Permanent(e.to_string()))?;
                        if message.topic != d.topic
                            || message.partition != Some(delivery.partition)
                            || message.offset != Some(delivery.offset)
                        {
                            return Err(StreamStageSourceError::Permanent(
                                "HTTP record metadata did not match delivery".into(),
                            ));
                        }
                        Ok(StreamStageRecord {
                            position: StreamPosition {
                                lane: StreamPositionLane {
                                    source: d.source.clone(),
                                    topic: d.topic.clone(),
                                    consumer_group: d.consumer_group.clone(),
                                    partition: delivery.partition,
                                },
                                offset: delivery.offset,
                            },
                            message,
                            receipt: HttpStageReceipt {
                                source: i,
                                session: session.clone(),
                                receipt_id: delivery.receipt_id,
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            }
        });
        let mut records = Vec::new();
        for batch in futures::future::join_all(requests).await {
            records.extend(batch?);
        }
        records.sort_by(|a, b| {
            (&a.position.lane, a.position.offset).cmp(&(&b.position.lane, b.position.offset))
        });
        Ok(records)
    }
    async fn validate(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<Vec<StreamPosition>, StreamStageSourceError> {
        let mut positions = Vec::new();
        for (i, ids) in self.grouped(records)? {
            let d = &self.definitions[i];
            let response = self
                .client
                .validate_bus_receipts(&d.namespace, &d.tenant, &d.id, &self.sessions[i], &ids)
                .await
                .map_err(|e| map_error(&e))?;
            if response.consumer_group != d.consumer_group {
                return Err(StreamStageSourceError::Fenced(
                    "validated group changed".into(),
                ));
            }
            positions.extend(response.positions.into_iter().map(|p| StreamPosition {
                lane: StreamPositionLane {
                    source: d.source.clone(),
                    topic: d.topic.clone(),
                    consumer_group: d.consumer_group.clone(),
                    partition: p.partition,
                },
                offset: p.offset,
            }));
        }
        Ok(positions)
    }
    async fn acknowledge(
        &mut self,
        records: &[StreamStageRecord<Self::Receipt>],
    ) -> Result<(), StreamStageSourceError> {
        self.validate(records).await?;
        for (i, ids) in self.grouped(records)? {
            let d = &self.definitions[i];
            self.client
                .acknowledge_bus_receipts(&d.namespace, &d.tenant, &d.id, &self.sessions[i], &ids)
                .await
                .map_err(|e| map_error(&e))?;
        }
        Ok(())
    }
    async fn recover(&mut self) -> Result<(), StreamStageSourceError> {
        self.close().await;
        Ok(())
    }
    async fn close(&mut self) {
        HttpStreamStageSource::close(self).await;
    }
}
fn map_error(error: &Error) -> StreamStageSourceError {
    match error {
        Error::Http {
            status: 404 | 409 | 410,
            ..
        } => StreamStageSourceError::Fenced(error.to_string()),
        Error::Http { status: 429, .. } => StreamStageSourceError::Retryable(error.to_string()),
        _ if error.is_retryable() => StreamStageSourceError::Retryable(error.to_string()),
        _ => StreamStageSourceError::Permanent(error.to_string()),
    }
}
