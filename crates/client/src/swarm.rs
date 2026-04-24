//! Swarm provider client helpers.
//!
//! These wrap the `/v1/swarm/runs` HTTP surface exposed when the server
//! is built with the `swarm` feature. Swarm goals themselves are dispatched
//! through the regular [`ActeonClient::dispatch`] surface with
//! `provider = "swarm"` and a payload matching the crate's `GoalRequest`
//! shape — the helpers here are for observing and controlling inflight
//! runs, not for creating them.

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Characters that must be escaped inside a path segment.
/// Everything non-unreserved per RFC 3986 plus `/`, `?`, `#` (which
/// would otherwise break the path boundary).
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'?')
    .add(b'/')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}');

fn encode_segment(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

/// Snapshot of a single swarm run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmRunSnapshot {
    /// Unique run identifier assigned by the server.
    pub run_id: String,
    /// Plan ID from the original goal.
    pub plan_id: String,
    /// Objective echoed from the goal.
    pub objective: String,
    /// Current status (e.g. `accepted`, `running`, `completed`).
    pub status: String,
    /// When the run was accepted by the provider (ISO-8601).
    pub started_at: String,
    /// When the run reached a terminal state (ISO-8601, if any).
    #[serde(default)]
    pub finished_at: Option<String>,
    /// Aggregate metrics collected by the swarm orchestrator.
    #[serde(default)]
    pub metrics: Option<serde_json::Value>,
    /// Error message if the run failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Namespace of the originating action.
    pub namespace: String,
    /// Tenant of the originating action.
    pub tenant: String,
}

/// Filter for listing swarm runs.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SwarmRunFilter {
    /// Restrict to a specific namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Restrict to a specific tenant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Restrict by status (`accepted`, `running`, `completed`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Maximum number of results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Number of results to skip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// Response from listing swarm runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSwarmRunsResponse {
    pub runs: Vec<SwarmRunSnapshot>,
    pub total: usize,
}

impl ActeonClient {
    /// List swarm runs tracked by the server-side registry.
    pub async fn list_swarm_runs(
        &self,
        filter: &SwarmRunFilter,
    ) -> Result<ListSwarmRunsResponse, Error> {
        let url = format!("{}/v1/swarm/runs", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ListSwarmRunsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list swarm runs".to_string(),
            })
        }
    }

    /// Fetch a single swarm run snapshot by ID.
    ///
    /// Returns `Ok(None)` if the run is unknown to the server.
    pub async fn get_swarm_run(&self, run_id: &str) -> Result<Option<SwarmRunSnapshot>, Error> {
        let encoded = encode_segment(run_id);
        let url = format!("{}/v1/swarm/runs/{encoded}", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SwarmRunSnapshot>()
                .await
                .map(Some)
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to fetch swarm run".to_string(),
            })
        }
    }

    /// Request cancellation of an inflight swarm run.
    ///
    /// Idempotent — returns the latest snapshot even if the run was
    /// already terminal. Returns `Ok(None)` if the run is unknown.
    pub async fn cancel_swarm_run(&self, run_id: &str) -> Result<Option<SwarmRunSnapshot>, Error> {
        let encoded = encode_segment(run_id);
        let url = format!("{}/v1/swarm/runs/{encoded}/cancel", self.base_url);
        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SwarmRunSnapshot>()
                .await
                .map(Some)
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            let status = response.status().as_u16();
            let err = response.json::<ErrorResponse>().await.ok();
            Err(Error::Http {
                status,
                message: err
                    .map_or_else(|| "Failed to cancel swarm run".to_string(), |e| e.message),
            })
        }
    }
}
