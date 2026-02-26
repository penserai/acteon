use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Summary of a chain for list responses.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainSummary {
    /// Unique chain ID.
    pub chain_id: String,
    /// Human-readable chain name.
    pub chain_name: String,
    /// Current status (e.g., "running", "completed", "failed", "cancelled").
    pub status: String,
    /// Index of the current step being executed.
    pub current_step: usize,
    /// Total number of steps in the chain.
    pub total_steps: usize,
    /// When the chain was started.
    pub started_at: String,
    /// When the chain was last updated.
    pub updated_at: String,
    /// Parent chain ID if this is a sub-chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_chain_id: Option<String>,
}

/// Response from listing chains.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListChainsResponse {
    /// List of chain summaries.
    pub chains: Vec<ChainSummary>,
}

/// Status of an individual chain step.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainStepStatus {
    /// Step name.
    pub name: String,
    /// Target provider for this step.
    pub provider: String,
    /// Step status: `"pending"`, `"running"`, `"completed"`, `"failed"`,
    /// `"skipped"`, `"waiting_sub_chain"`, or `"waiting_parallel"`. Parallel
    /// sub-steps may also report `"cancelled"`.
    pub status: String,
    /// Response body from the provider, if completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<serde_json::Value>,
    /// Error message, if the step failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the step completed, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Name of the sub-chain this step triggers, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_chain: Option<String>,
    /// ID of the child chain instance spawned by this step, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_chain_id: Option<String>,
    /// Results from parallel sub-steps, if this is a parallel step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_sub_steps: Option<Vec<ChainStepStatus>>,
}

/// Detailed response for a single chain.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainDetailResponse {
    /// Unique chain ID.
    pub chain_id: String,
    /// Human-readable chain name.
    pub chain_name: String,
    /// Current status.
    pub status: String,
    /// Index of the current step being executed.
    pub current_step: usize,
    /// Total number of steps in the chain.
    pub total_steps: usize,
    /// Per-step status details.
    pub steps: Vec<ChainStepStatus>,
    /// When the chain was started.
    pub started_at: String,
    /// When the chain was last updated.
    pub updated_at: String,
    /// When the chain expires, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Reason the chain was cancelled, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_reason: Option<String>,
    /// Who cancelled the chain, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_by: Option<String>,
    /// Ordered list of step names that were actually executed (for branching).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_path: Vec<String>,
    /// Parent chain ID if this is a sub-chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_chain_id: Option<String>,
    /// IDs of child chains spawned by sub-chain steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_chain_ids: Vec<String>,
}

/// A node in the chain DAG.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DagNode {
    /// Node name (step name or sub-chain name).
    pub name: String,
    /// Node type: `step` or `sub_chain`.
    pub node_type: String,
    /// Provider for this step, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Action type for this step, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Name of the sub-chain, if this is a sub-chain node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_chain_name: Option<String>,
    /// Current status of this node (for instance DAGs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// ID of the child chain instance (for instance DAGs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_chain_id: Option<String>,
    /// Nested DAG for sub-chain expansion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Box<DagResponse>>,
    /// Nested DAG nodes for parallel sub-steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_children: Option<Vec<DagNode>>,
    /// Join policy for parallel groups (`"all"` or `"any"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_join: Option<String>,
}

/// An edge in the chain DAG.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DagEdge {
    /// Source node name.
    pub source: String,
    /// Target node name.
    pub target: String,
    /// Edge label (e.g., branch condition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this edge is on the execution path (for instance DAGs).
    #[serde(default)]
    pub on_execution_path: bool,
}

/// DAG representation of a chain (config or instance).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DagResponse {
    /// Chain configuration name.
    pub chain_name: String,
    /// Chain instance ID (only for instance DAGs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Chain status (only for instance DAGs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Nodes in the DAG.
    pub nodes: Vec<DagNode>,
    /// Edges connecting the nodes.
    pub edges: Vec<DagEdge>,
    /// Ordered list of step names on the execution path (for instance DAGs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub execution_path: Vec<String>,
}

impl ActeonClient {
    /// List chains filtered by namespace, tenant, and optional status.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.list_chains("notifications", "tenant-1", Some("running")).await?;
    /// for chain in result.chains {
    ///     println!("{}: {} (step {}/{})", chain.chain_id, chain.chain_name, chain.current_step, chain.total_steps);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_chains(
        &self,
        namespace: &str,
        tenant: &str,
        status: Option<&str>,
    ) -> Result<ListChainsResponse, Error> {
        let url = format!("{}/v1/chains", self.base_url);

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
                .json::<ListChainsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list chains: {}", response.status()),
            })
        }
    }

    /// Get the full details of a chain by ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let detail = client.get_chain("chain-123", "notifications", "tenant-1").await?;
    /// println!("{}: {} steps", detail.chain_name, detail.total_steps);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_chain(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<ChainDetailResponse, Error> {
        let url = format!("{}/v1/chains/{}", self.base_url, chain_id);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ChainDetailResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Chain not found: {chain_id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get chain: {}", response.status()),
            })
        }
    }

    /// Cancel a running chain.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let detail = client.cancel_chain(
    ///     "chain-123",
    ///     "notifications",
    ///     "tenant-1",
    ///     Some("no longer needed"),
    ///     Some("admin@example.com"),
    /// ).await?;
    /// println!("Chain {} cancelled", detail.chain_id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn cancel_chain(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
        reason: Option<&str>,
        cancelled_by: Option<&str>,
    ) -> Result<ChainDetailResponse, Error> {
        let url = format!("{}/v1/chains/{}/cancel", self.base_url, chain_id);

        let mut body = serde_json::json!({
            "namespace": namespace,
            "tenant": tenant,
        });
        if let Some(r) = reason {
            body["reason"] = serde_json::Value::String(r.to_string());
        }
        if let Some(cb) = cancelled_by {
            body["cancelled_by"] = serde_json::Value::String(cb.to_string());
        }

        let response = self
            .add_auth(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ChainDetailResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Chain not found: {chain_id}"),
            })
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// Get the DAG representation for a running chain instance.
    ///
    /// Returns the directed acyclic graph of steps and sub-chains,
    /// including execution state and the path taken so far.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let dag = client.get_chain_dag("chain-123", "notifications", "tenant-1").await?;
    /// println!("DAG for {}: {} nodes", dag.chain_name, dag.nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_chain_dag(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<DagResponse, Error> {
        let url = format!("{}/v1/chains/{}/dag", self.base_url, chain_id);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DagResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Chain not found: {chain_id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get chain DAG: {}", response.status()),
            })
        }
    }

    /// Get the DAG representation for a chain definition (config only).
    ///
    /// Returns the directed acyclic graph of steps and sub-chains
    /// from the chain configuration, without any runtime state.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let dag = client.get_chain_definition_dag("order-pipeline").await?;
    /// println!("Definition DAG for {}: {} nodes", dag.chain_name, dag.nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_chain_definition_dag(&self, name: &str) -> Result<DagResponse, Error> {
        let url = format!("{}/v1/chains/definitions/{}/dag", self.base_url, name);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DagResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Chain definition not found: {name}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get chain definition DAG: {}", response.status()),
            })
        }
    }
}
