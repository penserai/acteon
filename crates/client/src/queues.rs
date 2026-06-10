use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ActeonClient, Error};

/// A worker task delivered through a named queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkerTask {
    /// Unique task ID.
    pub task_id: String,
    /// Queue the task is routed through.
    pub queue: String,
    /// Action type for worker handler dispatch.
    pub action_type: String,
    /// Task payload.
    pub payload: Value,
    /// Lifecycle status (`"pending"`, `"leased"`, `"completed"`, `"failed"`,
    /// `"cancelled"`).
    pub status: String,
    /// Delivery attempt (1-based once leased).
    pub attempt: u32,
    /// Maximum delivery attempts.
    pub max_attempts: u32,
    /// Lease token; required for heartbeat / complete / fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_token: Option<String>,
    /// When the current lease expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    /// Result reported by the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error reported on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Owning chain execution for chain `worker` steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Owning workflow execution for workflow continuation tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_execution_id: Option<String>,
    /// When the task was enqueued.
    pub created_at: String,
    /// When the task was last updated.
    pub updated_at: String,
}

/// Response from polling or listing a queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PollQueueResponse {
    /// Leased (or listed) tasks.
    pub tasks: Vec<WorkerTask>,
}

/// Options for polling a queue.
#[derive(Debug, Clone, Default)]
pub struct PollOptions {
    /// Maximum tasks to lease in one poll (default 1).
    pub max_tasks: Option<usize>,
    /// Lease duration in seconds (default 60, max 3600).
    pub lease_seconds: Option<u64>,
    /// Identifier of the polling worker.
    pub worker_id: Option<String>,
}

impl ActeonClient {
    /// Enqueue a task on a named worker queue.
    pub async fn enqueue_task(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
        action_type: &str,
        payload: Value,
        max_attempts: Option<u32>,
    ) -> Result<WorkerTask, Error> {
        let url = format!("{}/v1/queues/{queue}/tasks", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "action_type": action_type,
            "payload": payload,
            "max_attempts": max_attempts,
        });
        self.queue_post(&url, &body, "enqueue task").await
    }

    /// Lease pending tasks from a queue.
    pub async fn poll_tasks(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
        options: &PollOptions,
    ) -> Result<Vec<WorkerTask>, Error> {
        let url = format!("{}/v1/queues/{queue}/poll", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "max_tasks": options.max_tasks,
            "lease_seconds": options.lease_seconds,
            "worker_id": options.worker_id,
        });
        let response: PollQueueResponse = self.queue_post(&url, &body, "poll queue").await?;
        Ok(response.tasks)
    }

    /// Extend the lease on a task.
    pub async fn heartbeat_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        extend_seconds: Option<u64>,
    ) -> Result<WorkerTask, Error> {
        let url = format!("{}/v1/queues/tasks/{task_id}/heartbeat", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "lease_token": lease_token,
            "extend_seconds": extend_seconds,
        });
        self.queue_post(&url, &body, "heartbeat task").await
    }

    /// Complete a leased task with a result.
    pub async fn complete_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        result: Value,
    ) -> Result<WorkerTask, Error> {
        let url = format!("{}/v1/queues/tasks/{task_id}/complete", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "lease_token": lease_token,
            "result": result,
        });
        self.queue_post(&url, &body, "complete task").await
    }

    /// Fail a leased task. Retryable failures within the attempt budget are
    /// re-queued with backoff.
    pub async fn fail_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        lease_token: &str,
        error: &str,
        retryable: bool,
    ) -> Result<WorkerTask, Error> {
        let url = format!("{}/v1/queues/tasks/{task_id}/fail", self.base_url);
        let body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
            "lease_token": lease_token,
            "error": error,
            "retryable": retryable,
        });
        self.queue_post(&url, &body, "fail task").await
    }

    /// Get a worker task by ID.
    pub async fn get_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<WorkerTask, Error> {
        let url = format!("{}/v1/queues/tasks/{task_id}", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<WorkerTask>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get task: {}", response.status()),
            })
        }
    }

    /// List tasks on a queue, optionally filtered by status.
    pub async fn list_tasks(
        &self,
        namespace: &str,
        tenant: &str,
        queue: &str,
        status: Option<&str>,
    ) -> Result<Vec<WorkerTask>, Error> {
        let url = format!("{}/v1/queues/{queue}/tasks", self.base_url);
        let mut query: Vec<(&str, &str)> = vec![("namespace", namespace), ("tenant", tenant)];
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
            let result = response
                .json::<PollQueueResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result.tasks)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list tasks: {}", response.status()),
            })
        }
    }

    /// Shared POST-and-decode helper for queue endpoints.
    async fn queue_post<T: serde::de::DeserializeOwned>(
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
