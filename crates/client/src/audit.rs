use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Query parameters for audit search.
#[derive(Debug, Default, Clone, Serialize)]
pub struct AuditQuery {
    /// Filter by namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Filter by provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Filter by outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Maximum number of records to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Number of records to skip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

/// Paginated audit results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPage {
    /// Audit records.
    pub records: Vec<AuditRecord>,
    /// Total number of matching records.
    pub total: u64,
    /// Limit used in the query.
    pub limit: u64,
    /// Offset used in the query.
    pub offset: u64,
}

/// An audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Record ID.
    pub id: String,
    /// Action ID.
    pub action_id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Provider name.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// Rule verdict.
    pub verdict: String,
    /// Action outcome.
    pub outcome: String,
    /// Name of matched rule, if any.
    pub matched_rule: Option<String>,
    /// Processing duration in milliseconds.
    pub duration_ms: u64,
    /// Dispatch timestamp.
    pub dispatched_at: String,
    /// `SHA-256` hex digest of the canonicalized record content (compliance mode).
    #[serde(default)]
    pub record_hash: Option<String>,
    /// Hash of the previous record in the chain (compliance mode).
    #[serde(default)]
    pub previous_hash: Option<String>,
    /// Monotonic sequence number within the `(namespace, tenant)` pair (compliance mode).
    #[serde(default)]
    pub sequence_number: Option<u64>,
}

/// Query parameters for bulk audit replay.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ReplayQuery {
    /// Filter by namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Filter by tenant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Filter by provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Filter by action type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Filter by outcome (e.g., "failed", "suppressed").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Filter by verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Filter by matched rule name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    /// Only records dispatched at or after this time (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Only records dispatched at or before this time (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Maximum number of records to replay (default 50, max 1000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Result of replaying a single action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    /// The original action ID from the audit record.
    pub original_action_id: String,
    /// The new action ID assigned to the replayed action.
    pub new_action_id: String,
    /// Whether the replay succeeded.
    pub success: bool,
    /// Error message if the replay failed.
    pub error: Option<String>,
}

/// Summary of a bulk replay operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaySummary {
    /// Number of actions successfully replayed.
    pub replayed: usize,
    /// Number of actions that failed to replay.
    pub failed: usize,
    /// Number of records skipped (no stored payload).
    pub skipped: usize,
    /// Per-action results.
    pub results: Vec<ReplayResult>,
}

impl ActeonClient {
    /// Query audit records.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, AuditQuery};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = AuditQuery {
    ///     tenant: Some("tenant-1".to_string()),
    ///     limit: Some(10),
    ///     ..Default::default()
    /// };
    ///
    /// let page = client.query_audit(&query).await?;
    /// println!("Found {} records (total: {})", page.records.len(), page.total);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_audit(&self, query: &AuditQuery) -> Result<AuditPage, Error> {
        let url = format!("{}/v1/audit", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let page = response
                .json::<AuditPage>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(page)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to query audit: {}", response.status()),
            })
        }
    }

    /// Get a specific audit record by action ID.
    ///
    /// Returns `None` if the record is not found.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(record) = client.get_audit_record("action-id-123").await? {
    ///     println!("Found record: {:?}", record);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_audit_record(&self, action_id: &str) -> Result<Option<AuditRecord>, Error> {
        let url = format!("{}/v1/audit/{}", self.base_url, action_id);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let record = response
                .json::<AuditRecord>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(record))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get audit record: {}", response.status()),
            })
        }
    }

    /// Replay a single action from the audit trail by its action ID.
    ///
    /// Reconstructs the original action from the stored audit payload and
    /// dispatches it through the gateway pipeline with a new action ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.replay_action("action-id-123").await?;
    /// if result.success {
    ///     println!("Replayed as {}", result.new_action_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn replay_action(&self, action_id: &str) -> Result<ReplayResult, Error> {
        let url = format!("{}/v1/audit/{}/replay", self.base_url, action_id);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ReplayResult>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Audit record not found: {action_id}"),
            })
        } else if response.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            Err(Error::Http {
                status: 422,
                message: "No stored payload available for replay".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to replay action: {}", response.status()),
            })
        }
    }

    /// Bulk replay actions from the audit trail matching the given query.
    ///
    /// Returns a summary with per-action results.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, ReplayQuery};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = ReplayQuery {
    ///     outcome: Some("failed".to_string()),
    ///     limit: Some(100),
    ///     ..Default::default()
    /// };
    ///
    /// let summary = client.replay_audit(&query).await?;
    /// println!("Replayed: {}, Failed: {}, Skipped: {}", summary.replayed, summary.failed, summary.skipped);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn replay_audit(&self, query: &ReplayQuery) -> Result<ReplaySummary, Error> {
        let url = format!("{}/v1/audit/replay", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let summary = response
                .json::<ReplaySummary>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(summary)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to replay audit: {}", response.status()),
            })
        }
    }
}
