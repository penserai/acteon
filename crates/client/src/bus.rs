//! Client helpers for the agentic bus surface.
//!
//! Wraps the full HTTP surface (topics, subscriptions, schemas, agents,
//! conversations, tool-calls, streams, approvals) plus typed SSE
//! consumers for `/v1/bus/subscribe/{id}` (consume a subscription) and
//! `/v1/bus/streams/.../{stream_id}` (tail a single stream).

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::task::{Context, Poll};

use chrono::{DateTime, Utc};
use futures::stream::{Stream, StreamExt};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::stream::{SseEnvelope, sse_envelope_stream};
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
    #[serde(default)]
    pub schema_subject: Option<String>,
    #[serde(default)]
    pub schema_version: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LagPartition {
    pub partition: i32,
    pub committed: i64,
    pub high_water_mark: i64,
    pub lag: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    // ----- Phase 3: schemas + topic-schema binding -----

    /// Register a new version of a schema subject. The server allocates
    /// the next monotonic version and returns the registered schema.
    pub async fn register_bus_schema(&self, req: &RegisterBusSchema) -> Result<BusSchema, Error> {
        let url = format!("{}/v1/bus/schemas", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusSchema>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// List schemas, optionally filtered by namespace/tenant/subject.
    /// When `latest_only` is true, returns only the latest version per
    /// subject.
    pub async fn list_bus_schemas(
        &self,
        filter: &BusSchemaFilter,
    ) -> Result<ListBusSchemasResponse, Error> {
        let url = format!("{}/v1/bus/schemas", self.base_url);
        let resp = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusSchemasResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Fetch every version of a subject, ordered oldest-to-newest.
    pub async fn get_bus_schema_versions(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
    ) -> Result<ListBusSchemasResponse, Error> {
        let url = self.schema_subject_url(namespace, tenant, subject);
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusSchemasResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Fetch a specific schema version. Pass `"latest"` for the most
    /// recent; any numeric string is parsed as a version number.
    pub async fn get_bus_schema(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
        version: &str,
    ) -> Result<BusSchema, Error> {
        let url = self.schema_version_url(namespace, tenant, subject, version);
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusSchema>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Delete a schema version. Fails with 409 if any topic currently
    /// pins this version.
    pub async fn delete_bus_schema(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
        version: i32,
    ) -> Result<(), Error> {
        let url = self.schema_version_url(namespace, tenant, subject, &version.to_string());
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Bind a topic to a schema subject + version. The server
    /// validates every subsequent publish against this binding.
    pub async fn bind_topic_schema(
        &self,
        namespace: &str,
        tenant: &str,
        topic_name: &str,
        subject: &str,
        version: i32,
    ) -> Result<BindTopicSchemaResponse, Error> {
        let url = self.topic_schema_url(namespace, tenant, topic_name);
        let req = BindTopicSchemaRequest {
            subject: subject.to_string(),
            version,
        };
        let resp = self
            .add_auth(self.client.put(&url))
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BindTopicSchemaResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Drop a topic's schema binding. Publishes after this skip
    /// validation.
    pub async fn unbind_topic_schema(
        &self,
        namespace: &str,
        tenant: &str,
        topic_name: &str,
    ) -> Result<(), Error> {
        let url = self.topic_schema_url(namespace, tenant, topic_name);
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Convenience: serialize a typed value and publish it. Pair with
    /// a schema-bound topic for end-to-end type safety.
    pub async fn publish_typed<T: serde::Serialize>(
        &self,
        req: &PublishTyped<'_, T>,
    ) -> Result<PublishReceipt, Error> {
        let payload =
            serde_json::to_value(req.value).map_err(|e| Error::Deserialization(e.to_string()))?;
        let msg = PublishBusMessage {
            topic: req.topic.map(str::to_string),
            namespace: req.namespace.map(str::to_string),
            tenant: req.tenant.map(str::to_string),
            name: req.name.map(str::to_string),
            key: req.key.map(str::to_string),
            payload,
            headers: req.headers.clone(),
        };
        self.publish_message(&msg).await
    }

    fn schema_subject_url(&self, namespace: &str, tenant: &str, subject: &str) -> String {
        format!(
            "{}/v1/bus/schemas/{}/{}/{}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(subject)
        )
    }

    fn schema_version_url(
        &self,
        namespace: &str,
        tenant: &str,
        subject: &str,
        version: &str,
    ) -> String {
        format!(
            "{}/v1/bus/schemas/{}/{}/{}/{}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(subject),
            encode_segment(version)
        )
    }

    fn topic_schema_url(&self, namespace: &str, tenant: &str, topic_name: &str) -> String {
        format!(
            "{}/v1/bus/topics/{}/{}/{}/schema",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(topic_name)
        )
    }

    // ----- Phase 4: agents -----

    /// Register an agent. First agent in a `(namespace, tenant)`
    /// causes the shared inbox topic `{ns}.{tenant}.agents-inbox` to
    /// be auto-created.
    pub async fn register_bus_agent(&self, req: &RegisterBusAgent) -> Result<BusAgent, Error> {
        let url = format!("{}/v1/bus/agents", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusAgent>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// List agents, optionally filtered by namespace/tenant/capability/status.
    pub async fn list_bus_agents(
        &self,
        filter: &BusAgentFilter,
    ) -> Result<ListBusAgentsResponse, Error> {
        let url = format!("{}/v1/bus/agents", self.base_url);
        let resp = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusAgentsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Fetch a single agent record.
    pub async fn get_bus_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
    ) -> Result<BusAgent, Error> {
        let url = self.agent_url(namespace, tenant, agent_id, None);
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusAgent>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Patch-style update of the mutable fields on an agent record.
    pub async fn update_bus_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        req: &UpdateBusAgent,
    ) -> Result<BusAgent, Error> {
        let url = self.agent_url(namespace, tenant, agent_id, None);
        let resp = self
            .add_auth(self.client.put(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusAgent>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Delete an agent record. The shared inbox topic is left in
    /// place — other agents in the tenant may still need it.
    pub async fn delete_bus_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
    ) -> Result<(), Error> {
        let url = self.agent_url(namespace, tenant, agent_id, None);
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Record a heartbeat. Agents typically call this once per
    /// `heartbeat_ttl_ms / 3` to stay `Online`.
    pub async fn heartbeat_bus_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
    ) -> Result<BusAgentHeartbeat, Error> {
        let url = self.agent_url(namespace, tenant, agent_id, Some("heartbeat"));
        let resp = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusAgentHeartbeat>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Deliver a message to the agent's inbox. Keyed by `agent_id` so
    /// Kafka routes to a stable partition per agent.
    pub async fn send_to_bus_agent(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        req: &SendToBusAgent,
    ) -> Result<BusAgentSendReceipt, Error> {
        let url = self.agent_url(namespace, tenant, agent_id, Some("send"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusAgentSendReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    fn agent_url(
        &self,
        namespace: &str,
        tenant: &str,
        agent_id: &str,
        suffix: Option<&str>,
    ) -> String {
        let ns = encode_segment(namespace);
        let t = encode_segment(tenant);
        let a = encode_segment(agent_id);
        match suffix {
            Some(s) => format!("{}/v1/bus/agents/{ns}/{t}/{a}/{s}", self.base_url),
            None => format!("{}/v1/bus/agents/{ns}/{t}/{a}", self.base_url),
        }
    }

    // ----- Phase 5: conversations -----

    /// Register a conversation. First conversation in a `(namespace,
    /// tenant)` causes the shared events topic
    /// `{ns}.{tenant}.conversations-events` to be auto-created.
    pub async fn register_bus_conversation(
        &self,
        req: &RegisterBusConversation,
    ) -> Result<BusConversation, Error> {
        let url = format!("{}/v1/bus/conversations", self.base_url);
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversation>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// List conversations, optionally filtered by namespace/tenant/state/participant.
    pub async fn list_bus_conversations(
        &self,
        filter: &BusConversationFilter,
    ) -> Result<ListBusConversationsResponse, Error> {
        let url = format!("{}/v1/bus/conversations", self.base_url);
        let resp = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusConversationsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Fetch a single conversation record.
    pub async fn get_bus_conversation(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
    ) -> Result<BusConversation, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, None);
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversation>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Patch-style update of mutable fields. State transitions go
    /// through [`Self::transition_bus_conversation`] instead.
    pub async fn update_bus_conversation(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &UpdateBusConversation,
    ) -> Result<BusConversation, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, None);
        let resp = self
            .add_auth(self.client.put(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversation>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Delete a conversation record. The shared events topic is
    /// preserved — other conversations in the tenant share it.
    pub async fn delete_bus_conversation(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
    ) -> Result<(), Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, None);
        let resp = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Drive the conversation through its state machine. Server
    /// returns 409 if the requested transition is illegal for the
    /// current state.
    pub async fn transition_bus_conversation(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        transition: BusConversationTransition,
    ) -> Result<BusConversation, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("transition"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(&serde_json::json!({ "transition": transition }))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversation>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Append a message to the conversation thread. Keyed by
    /// `conversation_id` so Kafka routes all messages for this thread
    /// to a stable partition (per-thread FIFO).
    pub async fn append_bus_conversation_message(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &AppendBusConversationMessage,
    ) -> Result<BusConversationAppendReceipt, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("messages"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversationAppendReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Replay the message history for a conversation. The server
    /// reads from the shared events topic and filters on the
    /// server-stamped `acteon.conversation.id` header.
    pub async fn replay_bus_conversation_messages(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        params: &ReplayBusConversationParams,
    ) -> Result<BusConversationReplay, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("messages"));
        let resp = self
            .add_auth(self.client.get(&url))
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusConversationReplay>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    fn conversation_url(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        suffix: Option<&str>,
    ) -> String {
        let ns = encode_segment(namespace);
        let t = encode_segment(tenant);
        let c = encode_segment(conversation_id);
        match suffix {
            Some(s) => format!("{}/v1/bus/conversations/{ns}/{t}/{c}/{s}", self.base_url),
            None => format!("{}/v1/bus/conversations/{ns}/{t}/{c}", self.base_url),
        }
    }

    // ----- Phase 6a: tool-call envelopes -----

    /// Append a tool-call envelope to a conversation. The bus stamps
    /// `acteon.envelope.kind = tool_call`, `acteon.tool.call_id`,
    /// `acteon.correlation_id`, and `acteon.reply_to` headers so
    /// subscribers can route on the call without parsing the payload.
    pub async fn post_bus_tool_call(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &PostBusToolCall,
    ) -> Result<PostBusToolCallOutcome, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("tool-calls"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(map_error(resp).await);
        }
        // 202 → parked approval; 200 → produced. The server uses the
        // `require_approval` flag on the request to choose; the
        // status code is the load-bearing distinction here.
        if status == reqwest::StatusCode::ACCEPTED {
            resp.json::<BusApprovalParkedReceipt>()
                .await
                .map(PostBusToolCallOutcome::Parked)
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            resp.json::<BusToolEnvelopeReceipt>()
                .await
                .map(PostBusToolCallOutcome::Produced)
                .map_err(|e| Error::Deserialization(e.to_string()))
        }
    }

    /// Append a tool-result envelope. Carries the originating
    /// `call_id` so consumers (and `lookup_bus_tool_result`) can match
    /// it to the call.
    pub async fn post_bus_tool_result(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &PostBusToolResult,
    ) -> Result<BusToolEnvelopeReceipt, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("tool-results"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusToolEnvelopeReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Wait for a tool result matching `call_id`. The server scans
    /// the events topic with a `timeout_ms` budget (default 5000ms,
    /// max 30000ms). Use `params.conversation_id` if the result is
    /// expected to land in a different conversation than the call
    /// (`reply_to` pattern).
    pub async fn lookup_bus_tool_result(
        &self,
        namespace: &str,
        tenant: &str,
        call_id: &str,
        params: &BusToolResultLookupParams,
    ) -> Result<BusToolResultLookup, Error> {
        let url = format!(
            "{}/v1/bus/tool-calls/{}/{}/{}/result",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(call_id),
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusToolResultLookup>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Post a tool call and wait for its matching result on the same
    /// conversation. Wires the receipt's `cursor` directly into the
    /// lookup so the scan starts strictly *after* the call was
    /// produced — avoiding the race where the result lands between
    /// the post and a separate lookup, and the busy-cluster path
    /// where a cursor-less lookup defaults to scanning from the
    /// topic tail.
    ///
    /// Use this for the common request/response pattern. For
    /// `reply_to`-routed flows where the result lands in a different
    /// conversation, call [`Self::post_bus_tool_call`] then
    /// [`Self::lookup_bus_tool_result`] explicitly.
    pub async fn post_bus_tool_call_and_wait(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &PostBusToolCall,
        timeout_ms: Option<u64>,
    ) -> Result<BusToolResultLookup, Error> {
        let outcome = self
            .post_bus_tool_call(namespace, tenant, conversation_id, req)
            .await?;
        let receipt = match outcome {
            PostBusToolCallOutcome::Produced(r) => r,
            // A parked approval means no Kafka record exists yet.
            // The natural request/response wait would never resolve.
            // Surface a typed error so callers don't time out
            // mysteriously.
            PostBusToolCallOutcome::Parked(p) => {
                return Err(Error::Configuration(format!(
                    "tool-call parked for approval (id {}); cannot wait for result \
                     until an operator calls approve",
                    p.approval_id,
                )));
            }
        };
        // Phase 10: read-side identity is grant-derived now, no
        // longer a query parameter. The agent_id bound to the
        // caller's API-key grant is what the server uses for the
        // participant-ACL check.
        let _ = req; // suppress unused warning when caller didn't set sender
        self.lookup_bus_tool_result(
            namespace,
            tenant,
            &receipt.call_id,
            &BusToolResultLookupParams {
                conversation_id: receipt.conversation_id,
                cursor: Some(receipt.cursor),
                timeout_ms,
            },
        )
        .await
    }

    // ----- Phase 6b: streaming envelopes -----

    /// Append a stream-chunk envelope to a conversation. The bus
    /// stamps `acteon.envelope.kind = stream_chunk`, `acteon.stream.id`,
    /// and `acteon.stream.seq` headers so subscribers can header-filter
    /// without parsing the chunk body.
    pub async fn post_bus_stream_chunk(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &PostBusStreamChunk,
    ) -> Result<BusStreamEnvelopeReceipt, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("stream-chunks"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusStreamEnvelopeReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Append the terminal `stream_end` marker. Once produced, SSE
    /// consumers of `GET /v1/bus/streams/.../{stream_id}` close their
    /// connection on observing this record.
    pub async fn post_bus_stream_end(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        req: &PostBusStreamEnd,
    ) -> Result<BusStreamEnvelopeReceipt, Error> {
        let url = self.conversation_url(namespace, tenant, conversation_id, Some("stream-end"));
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusStreamEnvelopeReceipt>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    // ----- Phase 6c: pre-publish HITL approvals -----

    /// List parked approvals for a tenant. Filter by status or
    /// conversation via `params`.
    pub async fn list_bus_approvals(
        &self,
        namespace: &str,
        tenant: &str,
        params: &ListBusApprovalsParams,
    ) -> Result<ListBusApprovalsResponse, Error> {
        let url = format!(
            "{}/v1/bus/approvals/{}/{}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<ListBusApprovalsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Fetch a single approval by id.
    pub async fn get_bus_approval(
        &self,
        namespace: &str,
        tenant: &str,
        approval_id: &str,
    ) -> Result<BusApprovalView, Error> {
        let url = format!(
            "{}/v1/bus/approvals/{}/{}/{}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(approval_id),
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusApprovalView>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Approve a parked tool-call. The server produces the original
    /// envelope to Kafka with the same headers it would have stamped
    /// on a non-gated post, plus an `acteon.approval.id` audit
    /// header.
    pub async fn approve_bus_approval(
        &self,
        namespace: &str,
        tenant: &str,
        approval_id: &str,
        req: &BusApprovalDecisionRequest,
    ) -> Result<BusApprovalDecisionResponse, Error> {
        self.post_bus_approval_decision(namespace, tenant, approval_id, "approve", req)
            .await
    }

    /// Reject a parked tool-call. No Kafka record is produced.
    pub async fn reject_bus_approval(
        &self,
        namespace: &str,
        tenant: &str,
        approval_id: &str,
        req: &BusApprovalDecisionRequest,
    ) -> Result<BusApprovalDecisionResponse, Error> {
        self.post_bus_approval_decision(namespace, tenant, approval_id, "reject", req)
            .await
    }

    async fn post_bus_approval_decision(
        &self,
        namespace: &str,
        tenant: &str,
        approval_id: &str,
        verb: &str,
        req: &BusApprovalDecisionRequest,
    ) -> Result<BusApprovalDecisionResponse, Error> {
        let url = format!(
            "{}/v1/bus/approvals/{}/{}/{}/{verb}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(approval_id),
        );
        let resp = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<BusApprovalDecisionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// Build the SSE consume URL for a stream. Useful when the caller
    /// wants to plug it into a browser `EventSource`, `curl
    /// -N --header 'accept: text/event-stream'`, or any other
    /// SSE-aware client. Encodes path segments per the bus URL rules.
    #[must_use]
    pub fn bus_stream_consume_url(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        stream_id: &str,
    ) -> String {
        format!(
            "{}/v1/bus/streams/{}/{}/{}/{}",
            self.base_url,
            encode_segment(namespace),
            encode_segment(tenant),
            encode_segment(conversation_id),
            encode_segment(stream_id),
        )
    }
}

// ----- Phase 3: DTOs -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RegisterBusSchema {
    pub subject: String,
    pub namespace: String,
    pub tenant: String,
    pub body: serde_json::Value,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusSchema {
    pub subject: String,
    pub version: i32,
    pub namespace: String,
    pub tenant: String,
    pub body: serde_json::Value,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBusSchemasResponse {
    pub schemas: Vec<BusSchema>,
    pub count: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BusSchemaFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub latest_only: bool,
}

#[derive(Debug, Clone, Serialize)]
struct BindTopicSchemaRequest {
    subject: String,
    version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindTopicSchemaResponse {
    pub topic: String,
    pub subject: String,
    pub version: i32,
}

/// Typed publish envelope consumed by
/// [`ActeonClient::publish_typed`]. Borrow-based to avoid clones when
/// the caller already has a typed value.
#[derive(Debug)]
pub struct PublishTyped<'a, T: serde::Serialize> {
    pub value: &'a T,
    pub topic: Option<&'a str>,
    pub namespace: Option<&'a str>,
    pub tenant: Option<&'a str>,
    pub name: Option<&'a str>,
    pub key: Option<&'a str>,
    pub headers: BTreeMap<String, String>,
}

// ----- Phase 4: agent DTOs -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RegisterBusAgent {
    pub agent_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbox_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_ttl_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// `inbox_topic` is intentionally absent — once an agent is
/// registered, its inbox is fixed. Migrating to a different topic
/// would orphan in-flight messages; delete and re-register if you
/// need a different inbox.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UpdateBusAgent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_ttl_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusAgent {
    pub agent_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub inbox_topic: String,
    pub heartbeat_ttl_ms: i64,
    #[serde(default)]
    pub last_heartbeat_at: Option<String>,
    pub status: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBusAgentsResponse {
    pub agents: Vec<BusAgent>,
    pub count: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BusAgentFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusAgentHeartbeat {
    pub agent_id: String,
    pub last_heartbeat_at: String,
    pub status: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SendToBusAgent {
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusAgentSendReceipt {
    pub inbox_topic: String,
    pub agent_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
}

// ----- Phase 5: conversation DTOs -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RegisterBusConversation {
    pub conversation_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_topic: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// `events_topic` is intentionally absent — once a conversation is
/// registered, the topic it produces to is fixed. Migrating mid-thread
/// would split the message log across two topics; delete and
/// re-register if you need a different topic.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UpdateBusConversation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participants: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusConversationState {
    Active,
    Resolved,
    Archived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusConversationTransition {
    Resolve,
    Reopen,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConversation {
    pub conversation_id: String,
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub title: Option<String>,
    pub state: BusConversationState,
    #[serde(default)]
    pub participants: Vec<String>,
    pub events_topic: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBusConversationsResponse {
    pub conversations: Vec<BusConversation>,
    pub count: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BusConversationFilter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppendBusConversationMessage {
    pub payload: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConversationAppendReceipt {
    pub events_topic: String,
    pub conversation_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReplayBusConversationParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Resume token from a previous response's `cursor`. When set,
    /// `from` is ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConversationReplayMessage {
    pub partition: i32,
    pub offset: i64,
    #[serde(default)]
    pub key: Option<String>,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub timestamp: String,
}

/// Why the server-side replay loop terminated. `Complete` = thread
/// fully drained at scan time; `Limit` and `Timeout` = partial,
/// follow-up needed via `cursor`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusReplayExitReason {
    Complete,
    Limit,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConversationReplay {
    pub conversation_id: String,
    pub events_topic: String,
    pub messages: Vec<BusConversationReplayMessage>,
    pub exit_reason: BusReplayExitReason,
    /// `Some` when the scan is incomplete. Pass back as
    /// `ReplayBusConversationParams.cursor` to continue.
    #[serde(default)]
    pub cursor: Option<String>,
}

// ----- Phase 6a: tool-envelope DTOs -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PostBusToolCall {
    pub call_id: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    /// Phase 6c: gate this tool-call behind a human-in-the-loop
    /// approval. When true, the server parks the envelope under a
    /// `BusApproval` row and responds with `202 Accepted` plus the
    /// approval id; nothing reaches Kafka until an operator
    /// approves via [`ActeonClient::approve_bus_approval`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_approval: bool,
    /// Free-form rationale for the approval request. Persisted on
    /// the row for the operator UX. Capped at 4 KB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_reason: Option<String>,
    /// Override the default approval TTL (24h). Capped at 7d. Ignored
    /// when `require_approval` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusToolResultStatus {
    Ok,
    Error,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostBusToolResult {
    pub call_id: String,
    pub status: BusToolResultStatus,
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusToolEnvelopeReceipt {
    pub events_topic: String,
    pub conversation_id: String,
    pub call_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
    /// Opaque cursor pointing at this envelope's `partition`+`offset`.
    /// Pass it back as [`BusToolResultLookupParams::cursor`] so the
    /// lookup scans only messages produced strictly after this
    /// envelope (avoids the busy-cluster denial-of-service path of
    /// scanning topic history).
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BusToolResultLookupParams {
    /// Required: the conversation to scan. For `reply_to`-routed
    /// flows, set this to the `reply_to` conversation. (Defaulting
    /// to the tenant's shared events topic was unsafe — a
    /// custom-events-topic conversation would have been silently
    /// scanned at the wrong place.)
    pub conversation_id: String,
    /// Resume cursor from the originating call's
    /// [`BusToolEnvelopeReceipt::cursor`]. Strongly recommended:
    /// without one the lookup defaults to scanning from the tail of
    /// the topic, which races with the result landing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// How long to wait for a matching result. Default 5000ms; max 30000ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusToolResult {
    pub call_id: String,
    pub status: BusToolResultStatus,
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusToolResultLookup {
    pub call_id: String,
    pub events_topic: String,
    pub conversation_id: String,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
    pub result: BusToolResult,
}

// ----- Phase 6b: streaming-envelope DTOs -----

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PostBusStreamChunk {
    pub stream_id: String,
    pub chunk_seq: i64,
    #[serde(default)]
    pub body: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusStreamEndStatus {
    Complete,
    Aborted,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostBusStreamEnd {
    pub stream_id: String,
    pub chunk_seq: i64,
    pub status: BusStreamEndStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

// ----- Phase 6c: HITL approval DTOs -----

#[derive(Debug, Clone)]
pub enum PostBusToolCallOutcome {
    /// Posted directly: tool-call landed on Kafka.
    Produced(BusToolEnvelopeReceipt),
    /// Parked under a `BusApproval` row pending a human decision.
    Parked(BusApprovalParkedReceipt),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BusApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusApprovalParkedReceipt {
    pub approval_id: String,
    pub namespace: String,
    pub tenant: String,
    pub conversation_id: String,
    pub correlation_token: String,
    pub status: BusApprovalStatus,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusApprovalView {
    pub approval_id: String,
    pub namespace: String,
    pub tenant: String,
    pub conversation_id: String,
    pub correlation_token: String,
    pub envelope_kind: String,
    pub status: BusApprovalStatus,
    #[serde(default)]
    pub reason: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub decided_at: Option<String>,
    #[serde(default)]
    pub decision_note: Option<String>,
    #[serde(default)]
    pub produced_partition: Option<i32>,
    #[serde(default)]
    pub produced_offset: Option<i64>,
    #[serde(default)]
    pub produced_at: Option<String>,
    #[serde(default)]
    pub envelope: serde_json::Value,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ListBusApprovalsParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<BusApprovalStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBusApprovalsResponse {
    pub approvals: Vec<BusApprovalView>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusApprovalDecisionRequest {
    pub decided_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusApprovalDecisionResponse {
    pub approval: BusApprovalView,
    #[serde(default)]
    pub receipt: Option<BusToolEnvelopeReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusStreamEnvelopeReceipt {
    pub events_topic: String,
    pub conversation_id: String,
    pub stream_id: String,
    pub chunk_seq: i64,
    pub partition: i32,
    pub offset: i64,
    pub produced_at: String,
    /// Opaque resume cursor encoding `{partition: offset}`. Pass it
    /// to the SSE consumer as `?cursor=` to scan from strictly after
    /// this envelope.
    pub cursor: String,
}

async fn map_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let err = resp.json::<ErrorResponse>().await.ok();
    Error::Http {
        status,
        message: err.map_or_else(|| "bus API error".to_string(), |e| e.message),
    }
}

// =============================================================================
// SSE consumers — generic topic subscribe + typed stream-id tail
// =============================================================================

/// Query params for [`ActeonClient::consume_bus_subscription`].
#[derive(Debug, Default, Clone, Serialize)]
pub struct ConsumeBusTopic {
    /// Full Kafka topic name (`namespace.tenant.name`).
    pub topic: String,
    /// `earliest` or `latest` (default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// A single Kafka record observed by a bus subscription consumer.
/// Mirrors the wire shape of `acteon_bus::BusMessage` without taking
/// a dependency on the bus crate from the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusConsumedMessage {
    pub topic: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub partition: Option<i32>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
}

/// Yielded items from [`BusConsumeStream`]. The `bus.message` event
/// becomes [`BusConsumeItem::Message`]; `bus.error` becomes
/// [`BusConsumeItem::Error`]; SSE comments surface as [`BusConsumeItem::KeepAlive`]
/// so consumers can use them as a liveness signal.
#[derive(Debug)]
pub enum BusConsumeItem {
    /// A consumed Kafka record.
    Message(Box<BusConsumedMessage>),
    /// Server-side error event (`bus.error`).
    Error { message: String },
    /// SSE keep-alive comment.
    KeepAlive,
}

/// Async stream of [`BusConsumeItem`]s from the SSE subscription
/// endpoint. Created via [`ActeonClient::consume_bus_subscription`].
pub struct BusConsumeStream {
    inner: Pin<Box<dyn Stream<Item = Result<BusConsumeItem, Error>> + Send>>,
}

impl Stream for BusConsumeStream {
    type Item = Result<BusConsumeItem, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Yielded items from [`BusStreamConsumeStream`]. Closes the underlying
/// HTTP connection after a [`BusStreamItem::End`] is observed.
#[derive(Debug)]
pub enum BusStreamItem {
    /// A typed `StreamChunk` envelope.
    Chunk(Box<acteon_core::StreamChunk>),
    /// Terminal `StreamEnd` marker. Stream closes after this.
    End(Box<acteon_core::StreamEnd>),
    /// Server-side error event (`bus.stream.error`).
    Error { message: String },
    /// SSE keep-alive comment.
    KeepAlive,
}

/// Async stream of [`BusStreamItem`]s from the per-stream SSE endpoint.
/// Created via [`ActeonClient::consume_bus_stream`].
pub struct BusStreamConsumeStream {
    inner: Pin<Box<dyn Stream<Item = Result<BusStreamItem, Error>> + Send>>,
}

impl Stream for BusStreamConsumeStream {
    type Item = Result<BusStreamItem, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl ActeonClient {
    /// Consume a bus subscription via SSE
    /// (`GET /v1/bus/subscribe/{subscription_id}`). Yields one item
    /// per Kafka record on the underlying topic.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, BusConsumeItem, ConsumeBusTopic};
    /// use futures::StreamExt;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let mut stream = client
    ///     .consume_bus_subscription(
    ///         "agent-A",
    ///         &ConsumeBusTopic {
    ///             topic: "agents.demo.events".into(),
    ///             from: Some("earliest".into()),
    ///         },
    ///     )
    ///     .await?;
    /// while let Some(item) = stream.next().await {
    ///     match item? {
    ///         BusConsumeItem::Message(msg) => println!("offset {:?}", msg.offset),
    ///         BusConsumeItem::Error { message } => eprintln!("server error: {message}"),
    ///         BusConsumeItem::KeepAlive => {}
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn consume_bus_subscription(
        &self,
        subscription_id: &str,
        params: &ConsumeBusTopic,
    ) -> Result<BusConsumeStream, Error> {
        let url = format!(
            "{}/v1/bus/subscribe/{}",
            self.base_url,
            encode_segment(subscription_id),
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(map_error(resp).await);
        }
        let inner = sse_envelope_stream(resp).map(parse_bus_subscribe_envelope);
        Ok(BusConsumeStream {
            inner: Box::pin(inner),
        })
    }

    /// Consume a typed stream via SSE
    /// (`GET /v1/bus/streams/{ns}/{tenant}/{conversation_id}/{stream_id}`).
    /// Server filters records by `(envelope_kind, conversation_id,
    /// stream_id)` so this stream only emits chunks for the requested
    /// stream id and closes after the terminal `StreamEnd`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, BusStreamItem};
    /// use futures::StreamExt;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let mut stream = client
    ///     .consume_bus_stream("agents", "demo", "thread-1", "stream-42")
    ///     .await?;
    /// while let Some(item) = stream.next().await {
    ///     match item? {
    ///         BusStreamItem::Chunk(c) => println!("chunk {} len {}", c.chunk_seq, c.body.to_string().len()),
    ///         BusStreamItem::End(e) => {
    ///             println!("stream ended: {:?}", e.status);
    ///             break;
    ///         }
    ///         BusStreamItem::Error { message } => eprintln!("server error: {message}"),
    ///         BusStreamItem::KeepAlive => {}
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn consume_bus_stream(
        &self,
        namespace: &str,
        tenant: &str,
        conversation_id: &str,
        stream_id: &str,
    ) -> Result<BusStreamConsumeStream, Error> {
        let url = self.bus_stream_consume_url(namespace, tenant, conversation_id, stream_id);
        let resp = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(map_error(resp).await);
        }
        let inner = sse_envelope_stream(resp).map(parse_bus_stream_envelope);
        Ok(BusStreamConsumeStream {
            inner: Box::pin(inner),
        })
    }
}

fn parse_bus_subscribe_envelope(
    envelope: Result<SseEnvelope, Error>,
) -> Result<BusConsumeItem, Error> {
    match envelope? {
        SseEnvelope::Frame(frame) => {
            let event = frame.event.as_deref().unwrap_or("message");
            match event {
                "bus.message" | "message" => {
                    let msg: BusConsumedMessage =
                        serde_json::from_str(&frame.data).map_err(|e| {
                            Error::Deserialization(format!("invalid bus.message payload: {e}"))
                        })?;
                    Ok(BusConsumeItem::Message(Box::new(msg)))
                }
                "bus.error" => Ok(BusConsumeItem::Error {
                    message: extract_error_message(&frame.data),
                }),
                other => Err(Error::Deserialization(format!(
                    "unexpected SSE event '{other}' on bus subscribe stream"
                ))),
            }
        }
        SseEnvelope::KeepAlive => Ok(BusConsumeItem::KeepAlive),
    }
}

fn parse_bus_stream_envelope(envelope: Result<SseEnvelope, Error>) -> Result<BusStreamItem, Error> {
    match envelope? {
        SseEnvelope::Frame(frame) => {
            let event = frame.event.as_deref().unwrap_or("message");
            match event {
                "bus.stream.chunk" => {
                    let chunk: acteon_core::StreamChunk = serde_json::from_str(&frame.data)
                        .map_err(|e| {
                            Error::Deserialization(format!("invalid stream chunk payload: {e}"))
                        })?;
                    Ok(BusStreamItem::Chunk(Box::new(chunk)))
                }
                "bus.stream.end" => {
                    let end: acteon_core::StreamEnd =
                        serde_json::from_str(&frame.data).map_err(|e| {
                            Error::Deserialization(format!("invalid stream end payload: {e}"))
                        })?;
                    Ok(BusStreamItem::End(Box::new(end)))
                }
                "bus.stream.error" => Ok(BusStreamItem::Error {
                    message: extract_error_message(&frame.data),
                }),
                other => Err(Error::Deserialization(format!(
                    "unexpected SSE event '{other}' on bus stream consumer"
                ))),
            }
        }
        SseEnvelope::KeepAlive => Ok(BusStreamItem::KeepAlive),
    }
}

fn extract_error_message(data: &str) -> String {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(ToString::to_string))
        .unwrap_or_else(|| data.to_string())
}

#[cfg(test)]
mod consumer_tests {
    use super::*;

    #[test]
    fn parse_subscribe_message_event() {
        let frame = crate::stream::SseFrame {
            event: Some("bus.message".into()),
            id: Some("42".into()),
            data: r#"{"topic":"agents.demo.events","payload":{"k":"v"},"partition":0,"offset":42}"#
                .into(),
        };
        let item = parse_bus_subscribe_envelope(Ok(SseEnvelope::Frame(frame))).unwrap();
        match item {
            BusConsumeItem::Message(m) => {
                assert_eq!(m.topic, "agents.demo.events");
                assert_eq!(m.offset, Some(42));
            }
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn parse_subscribe_error_event() {
        let frame = crate::stream::SseFrame {
            event: Some("bus.error".into()),
            id: None,
            data: r#"{"error":"broker disconnected"}"#.into(),
        };
        let item = parse_bus_subscribe_envelope(Ok(SseEnvelope::Frame(frame))).unwrap();
        match item {
            BusConsumeItem::Error { message } => assert_eq!(message, "broker disconnected"),
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn parse_subscribe_keep_alive() {
        let item = parse_bus_subscribe_envelope(Ok(SseEnvelope::KeepAlive)).unwrap();
        assert!(matches!(item, BusConsumeItem::KeepAlive));
    }

    #[test]
    fn parse_stream_chunk_and_end() {
        let chunk_frame = crate::stream::SseFrame {
            event: Some("bus.stream.chunk".into()),
            id: Some("0".into()),
            data: r#"{"stream_id":"s1","chunk_seq":3,"body":{"token":"hi"},"created_at":"2026-05-02T12:00:00Z"}"#
                .into(),
        };
        let end_frame = crate::stream::SseFrame {
            event: Some("bus.stream.end".into()),
            id: Some("1".into()),
            data: r#"{"stream_id":"s1","chunk_seq":4,"status":"complete","created_at":"2026-05-02T12:00:01Z"}"#
                .into(),
        };
        match parse_bus_stream_envelope(Ok(SseEnvelope::Frame(chunk_frame))).unwrap() {
            BusStreamItem::Chunk(c) => {
                assert_eq!(c.stream_id, "s1");
                assert_eq!(c.chunk_seq, 3);
            }
            other => panic!("unexpected item: {other:?}"),
        }
        match parse_bus_stream_envelope(Ok(SseEnvelope::Frame(end_frame))).unwrap() {
            BusStreamItem::End(e) => {
                assert_eq!(e.stream_id, "s1");
            }
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_error_event_with_plain_data() {
        // Server emits `{"error": "..."}`, but if the JSON is malformed
        // for some reason we still want a useful message back.
        let frame = crate::stream::SseFrame {
            event: Some("bus.stream.error".into()),
            id: None,
            data: "broker disconnected".into(),
        };
        let item = parse_bus_stream_envelope(Ok(SseEnvelope::Frame(frame))).unwrap();
        match item {
            BusStreamItem::Error { message } => assert_eq!(message, "broker disconnected"),
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn parse_stream_unknown_event_is_error() {
        let frame = crate::stream::SseFrame {
            event: Some("bogus".into()),
            id: None,
            data: "{}".into(),
        };
        let err = parse_bus_stream_envelope(Ok(SseEnvelope::Frame(frame))).unwrap_err();
        assert!(matches!(err, Error::Deserialization(_)));
    }
}
