use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ActeonClient, Error};

/// A recorded workflow checkpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowCheckpoint {
    /// Sequence number within the execution.
    pub seq: u64,
    /// Unique checkpoint name.
    pub name: String,
    /// Recorded payload.
    pub data: Value,
    /// When the checkpoint was recorded.
    pub recorded_at: String,
}

/// A workflow execution.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowExecution {
    /// Unique execution ID.
    pub execution_id: String,
    /// Workflow name.
    pub workflow: String,
    /// Worker queue continuation tasks are routed through.
    pub queue: String,
    /// Lifecycle status (`"running"`, `"waiting_timer"`, `"waiting_signal"`,
    /// `"completed"`, `"failed"`, `"cancelled"`).
    pub status: String,
    /// Input the execution started with.
    pub input: Value,
    /// Result (when completed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (when failed/cancelled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Recorded checkpoints.
    #[serde(default)]
    pub checkpoints: Vec<WorkflowCheckpoint>,
    /// What the execution is waiting on, when suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<Value>,
    /// Parent execution ID for child workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// IDs of child executions.
    #[serde(default)]
    pub children: Vec<String>,
    /// User-defined search attributes.
    #[serde(default)]
    pub search_attributes: HashMap<String, Value>,
    /// When the execution started.
    pub created_at: String,
    /// When the execution was last updated.
    pub updated_at: String,
}

/// Response from listing workflow executions.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListWorkflowsResponse {
    /// Matching executions, most recently started first.
    pub executions: Vec<WorkflowExecution>,
}

/// Response from recording a checkpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecordCheckpointResponse {
    /// Checkpoint name.
    pub name: String,
    /// Sequence number within the execution.
    pub seq: u64,
    /// Recorded payload (the original data on idempotent replays).
    pub data: Value,
}

impl ActeonClient {
    /// Start a workflow execution. Workflow code runs on external workers
    /// polling `queue`.
    pub async fn start_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        workflow: &str,
        queue: &str,
        input: Value,
        search_attributes: Option<HashMap<String, Value>>,
    ) -> Result<WorkflowExecution, Error> {
        let url = format!("{}/v1/workflows/start", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "workflow": workflow,
            "queue": queue,
            "input": input,
            "search_attributes": search_attributes.unwrap_or_default(),
        });
        self.workflow_post(&url, &body, "start workflow").await
    }

    /// List workflow executions.
    pub async fn list_workflows(
        &self,
        namespace: &str,
        tenant: &str,
        workflow: Option<&str>,
        status: Option<&str>,
    ) -> Result<ListWorkflowsResponse, Error> {
        let url = format!("{}/v1/workflows/executions", self.base_url);
        let mut query: Vec<(&str, &str)> = vec![("namespace", namespace), ("tenant", tenant)];
        if let Some(w) = workflow {
            query.push(("workflow", w));
        }
        if let Some(s) = status {
            query.push(("status", s));
        }
        let response = self
            .add_auth(self.client.get(&url))
            .query(&query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ListWorkflowsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list workflows: {}", response.status()),
            })
        }
    }

    /// Get a workflow execution.
    pub async fn get_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
    ) -> Result<WorkflowExecution, Error> {
        let url = format!("{}/v1/workflows/executions/{execution_id}", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<WorkflowExecution>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get workflow: {}", response.status()),
            })
        }
    }

    /// Deliver a signal to a workflow execution.
    pub async fn signal_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        signal_name: &str,
        payload: Option<Value>,
    ) -> Result<(), Error> {
        let url = format!(
            "{}/v1/workflows/executions/{execution_id}/signal/{signal_name}",
            self.base_url
        );
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "payload": payload,
        });
        let _: Value = self.workflow_post(&url, &body, "signal workflow").await?;
        Ok(())
    }

    /// Cancel a workflow execution.
    pub async fn cancel_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        reason: Option<&str>,
    ) -> Result<WorkflowExecution, Error> {
        let url = format!(
            "{}/v1/workflows/executions/{execution_id}/cancel",
            self.base_url
        );
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "reason": reason,
        });
        self.workflow_post(&url, &body, "cancel workflow").await
    }

    /// Record a checkpoint on a running execution (idempotent by name).
    /// Used by worker SDKs after each completed workflow step.
    pub async fn record_workflow_checkpoint(
        &self,
        namespace: &str,
        tenant: &str,
        execution_id: &str,
        name: &str,
        data: Value,
    ) -> Result<RecordCheckpointResponse, Error> {
        let url = format!(
            "{}/v1/workflows/executions/{execution_id}/checkpoints",
            self.base_url
        );
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "name": name,
            "data": data,
        });
        self.workflow_post(&url, &body, "record checkpoint").await
    }

    /// Start a child workflow, idempotently keyed by `checkpoint`. Returns
    /// the child execution ID. The child's terminal result is delivered to
    /// the parent as the signal `__child:{child_id}`.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_child_workflow(
        &self,
        namespace: &str,
        tenant: &str,
        parent_execution_id: &str,
        checkpoint: &str,
        workflow: &str,
        queue: Option<&str>,
        input: Value,
        parent_close_policy: Option<&str>,
    ) -> Result<String, Error> {
        let url = format!(
            "{}/v1/workflows/executions/{parent_execution_id}/children",
            self.base_url
        );
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "checkpoint": checkpoint,
            "workflow": workflow,
            "queue": queue,
            "input": input,
            "parent_close_policy": parent_close_policy,
        });
        let response: Value = self.workflow_post(&url, &body, "start child workflow").await?;
        response["child_execution_id"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                Error::Deserialization("missing child_execution_id in response".into())
            })
    }

    /// Shared POST-and-decode helper for workflow endpoints.
    async fn workflow_post<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        body: &Value,
        context: &str,
    ) -> Result<T, Error> {
        let response = self
            .add_auth(self.client.post(url))
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<T>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to {context}: {}", response.status()),
            })
        }
    }
}
