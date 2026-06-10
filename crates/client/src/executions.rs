use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ActeonClient, Error};

/// Summary of one execution for visibility queries.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionSummary {
    /// Unique execution ID.
    pub execution_id: String,
    /// Name of the chain definition.
    pub chain_name: String,
    /// Definition version pinned by this execution.
    pub version: u64,
    /// Current status (e.g. `"running"`, `"waiting_signal"`, `"completed"`).
    pub status: String,
    /// Current step index (0-based).
    pub current_step: usize,
    /// Total number of steps.
    pub total_steps: usize,
    /// When the execution started.
    pub started_at: String,
    /// When the execution was last updated.
    pub updated_at: String,
    /// User-defined search attributes.
    #[serde(default)]
    pub search_attributes: HashMap<String, Value>,
    /// What the execution is waiting on (timer / signal / worker), if paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_state: Option<Value>,
    /// Parent execution ID for sub-chains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_execution_id: Option<String>,
}

/// Response from listing executions.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListExecutionsResponse {
    /// Matching executions, most recently started first.
    pub executions: Vec<ExecutionSummary>,
}

/// One event in an execution's history log.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionHistoryEvent {
    /// Monotonic 1-based sequence number.
    pub event_id: u64,
    /// When the event was recorded.
    pub timestamp: String,
    /// Event discriminator (e.g. `"execution_started"`, `"timer_fired"`,
    /// `"signal_received"`, `"execution_completed"`).
    pub event_type: String,
    /// Remaining event-specific fields.
    #[serde(flatten)]
    pub fields: HashMap<String, Value>,
}

/// Response carrying an execution's history.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionHistoryResponse {
    /// Execution ID.
    pub execution_id: String,
    /// Ordered history events.
    pub events: Vec<ExecutionHistoryEvent>,
}

/// Filters for listing executions.
#[derive(Debug, Clone, Default)]
pub struct ExecutionListOptions {
    /// Only executions of this chain definition.
    pub chain_name: Option<String>,
    /// Only executions in this status.
    pub status: Option<String>,
    /// Only executions started at or after this RFC 3339 time.
    pub started_after: Option<String>,
    /// Only executions started at or before this RFC 3339 time.
    pub started_before: Option<String>,
    /// Search-attribute filter as `key=value`.
    pub attr: Option<String>,
    /// Maximum number of executions to return.
    pub limit: Option<usize>,
}

impl ActeonClient {
    /// List executions (including terminal ones) with optional filters.
    pub async fn list_executions(
        &self,
        namespace: &str,
        tenant: &str,
        options: &ExecutionListOptions,
    ) -> Result<ListExecutionsResponse, Error> {
        let url = format!("{}/v1/executions", self.base_url);
        let mut query: Vec<(&str, String)> = vec![
            ("namespace", namespace.to_owned()),
            ("tenant", tenant.to_owned()),
        ];
        if let Some(ref v) = options.chain_name {
            query.push(("chain_name", v.clone()));
        }
        if let Some(ref v) = options.status {
            query.push(("status", v.clone()));
        }
        if let Some(ref v) = options.started_after {
            query.push(("started_after", v.clone()));
        }
        if let Some(ref v) = options.started_before {
            query.push(("started_before", v.clone()));
        }
        if let Some(ref v) = options.attr {
            query.push(("attr", v.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }

        let response = self
            .add_auth(self.client.get(&url))
            .query(&query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ListExecutionsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list executions: {}", response.status()),
            })
        }
    }

    /// Get one execution's visibility summary.
    pub async fn get_execution(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<ExecutionSummary, Error> {
        let url = format!("{}/v1/executions/{execution_id}", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ExecutionSummary>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get execution: {}", response.status()),
            })
        }
    }

    /// Get an execution's full event history (works for chain executions
    /// and workflow executions alike).
    pub async fn get_execution_history(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<ExecutionHistoryResponse, Error> {
        let url = format!("{}/v1/executions/{execution_id}/history", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ExecutionHistoryResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get execution history: {}", response.status()),
            })
        }
    }

    /// Deliver an external signal to a chain execution. The signal payload
    /// becomes the wait step's response body.
    pub async fn signal_execution(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        signal_name: &str,
        payload: Option<Value>,
    ) -> Result<(), Error> {
        let url = format!(
            "{}/v1/executions/{execution_id}/signal/{signal_name}",
            self.base_url
        );
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "payload": payload,
        });
        let response = self
            .add_auth(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to signal execution: {}", response.status()),
            })
        }
    }

    /// Merge search attributes into an execution (existing keys are
    /// overwritten).
    pub async fn upsert_execution_attributes(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        attributes: HashMap<String, Value>,
    ) -> Result<ExecutionSummary, Error> {
        let url = format!("{}/v1/executions/{execution_id}/attributes", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "attributes": attributes,
        });
        let response = self
            .add_auth(self.client.put(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ExecutionSummary>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!(
                    "Failed to upsert execution attributes: {}",
                    response.status()
                ),
            })
        }
    }
}

impl ActeonClient {
    /// Reset a chain execution to re-run from an earlier step. Works on
    /// terminal executions; step results from the reset point onward are
    /// discarded.
    pub async fn reset_execution(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        step: &str,
        reason: Option<&str>,
    ) -> Result<ExecutionSummary, Error> {
        let url = format!("{}/v1/executions/{execution_id}/reset", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "step": step,
            "reason": reason,
        });
        let response = self
            .add_auth(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ExecutionSummary>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to reset execution: {}", response.status()),
            })
        }
    }
}
