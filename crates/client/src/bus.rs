//! Client helpers for the agentic bus surface (Phase 1).
//!
//! Wraps `/v1/bus/topics` CRUD and `/v1/bus/publish`. The SSE
//! subscribe stream is not wrapped here yet — Phase 2 will expose a
//! typed streaming consumer once subscriptions become first-class on
//! the server side.

use std::collections::{BTreeMap, HashMap};

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'?')
    .add(b'/')
    .add(b'%')
    .add(b'<')
    .add(b'>');

fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

/// Request body for creating a bus topic.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CreateBusTopic {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partitions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replication_factor: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusTopic {
    pub name: String,
    pub namespace: String,
    pub tenant: String,
    pub kafka_name: String,
    pub partitions: i32,
    pub replication_factor: i16,
    #[serde(default)]
    pub retention_ms: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListBusTopicsResponse {
    pub topics: Vec<BusTopic>,
    pub count: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BusTopicFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
}

/// Envelope handed to [`ActeonClient::publish_message`].
#[derive(Debug, Default, Clone, Serialize)]
pub struct PublishBusMessage {
    /// Either the full `namespace.tenant.name` form...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// ...or the three parts spelled out separately.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublishReceipt {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
}

// ----- Phase 2: subscriptions -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CreateSubscription {
    pub id: String,
    pub topic: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_offset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_letter_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusSubscription {
    pub id: String,
    pub topic: String,
    pub namespace: String,
    pub tenant: String,
    pub starting_offset: String,
    pub ack_mode: String,
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    pub ack_timeout_ms: u64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListBusSubscriptionsResponse {
    pub subscriptions: Vec<BusSubscription>,
    pub count: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BusSubscriptionFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AckOffset {
    pub partition: i32,
    pub offset: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LagPartition {
    pub partition: i32,
    pub committed: i64,
    pub high_water_mark: i64,
    pub lag: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BusLag {
    pub subscription_id: String,
    pub topic: String,
    pub partitions: Vec<LagPartition>,
    pub total_lag: i64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct DeadLetterRequest {
    pub partition: i32,
    pub offset: i64,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterReceipt {
    pub dlq_topic: String,
    pub partition: i32,
    pub offset: i64,
}

impl ActeonClient {
    /// Create a bus topic (persists in Acteon state and creates the
    /// backing Kafka topic).
    pub async fn create_bus_topic(&self, req: &CreateBusTopic) -> Result<BusTopic, Error> {
        let url = format!("{}/v1/bus/topics", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusTopic>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// List bus topics.
    pub async fn list_bus_topics(
        &self,
        filter: &BusTopicFilter,
    ) -> Result<ListBusTopicsResponse, Error> {
        let url = format!("{}/v1/bus/topics", self.base_url);
        let resp = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusTopicsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Delete a bus topic by Kafka name (`namespace.tenant.name`).
    pub async fn delete_bus_topic(&self, kafka_name: &str) -> Result<(), Error> {
        let encoded = encode_segment(kafka_name);
        let url = format!("{}/v1/bus/topics/{encoded}", self.base_url);
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT || status == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Publish a single message to a bus topic.
    pub async fn publish_message(&self, msg: &PublishBusMessage) -> Result<PublishReceipt, Error> {
        let url = format!("{}/v1/bus/publish", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(msg)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<PublishReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    // ---------------- Phase 2 subscription helpers ----------------

    /// Create a durable subscription (Kafka consumer group).
    pub async fn create_bus_subscription(
        &self,
        req: &CreateSubscription,
    ) -> Result<BusSubscription, Error> {
        let url = format!("{}/v1/bus/subscriptions", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusSubscription>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// List durable subscriptions.
    pub async fn list_bus_subscriptions(
        &self,
        filter: &BusSubscriptionFilter,
    ) -> Result<ListBusSubscriptionsResponse, Error> {
        let url = format!("{}/v1/bus/subscriptions", self.base_url);
        let resp = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusSubscriptionsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Delete a subscription. The `(namespace, tenant, id)` triple
    /// is used for an O(1) state-store lookup — no cross-tenant scan.
    pub async fn delete_bus_subscription(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<(), Error> {
        let url = self.subscription_url(namespace, tenant, id, None);
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT || status == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Commit an offset on behalf of a subscription's consumer group.
    ///
    /// **Performance warning**: this endpoint performs a full Kafka
    /// JoinGroup/SyncGroup round-trip on each call (hundreds of
    /// milliseconds on a warm broker). It is **not** suitable for
    /// per-record acks in a high-throughput workload — use it for
    /// end-of-batch checkpoints only. A future phase introduces a
    /// stateful subscription registry that collapses this overhead
    /// to microseconds.
    pub async fn ack_bus_subscription(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        position: AckOffset,
    ) -> Result<(), Error> {
        let url = self.subscription_url(namespace, tenant, id, Some("ack"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(&position)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Report per-partition lag for a subscription's consumer group.
    pub async fn get_bus_lag(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
    ) -> Result<BusLag, Error> {
        let url = self.subscription_url(namespace, tenant, id, Some("lag"));
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusLag>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Route a failed record to the subscription's configured dead-letter
    /// topic.
    pub async fn deadletter_bus_message(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        req: &DeadLetterRequest,
    ) -> Result<DeadLetterReceipt, Error> {
        let url = self.subscription_url(namespace, tenant, id, Some("deadletter"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<DeadLetterReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    fn subscription_url(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        suffix: Option<&str>,
    ) -> String {
        let ns = encode_segment(namespace);
        let t = encode_segment(tenant);
        let i = encode_segment(id);
        match suffix {
            Some(s) => format!("{}/v1/bus/subscriptions/{ns}/{t}/{i}/{s}", self.base_url),
            None => format!("{}/v1/bus/subscriptions/{ns}/{t}/{i}", self.base_url),
        }
    }
}

async fn map_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let err = resp.json::<ErrorResponse>().await.ok();
    Error::Http {
        status,
        message: err.map_or_else(|| "bus API error".to_string(), |e| e.message),
    }
}
