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
}

async fn map_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let err = resp.json::<ErrorResponse>().await.ok();
    Error::Http {
        status,
        message: err.map_or_else(|| "bus API error".to_string(), |e| e.message),
    }
}
