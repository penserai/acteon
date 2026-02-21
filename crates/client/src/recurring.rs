use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Request to create a recurring action.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CreateRecurringAction {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Target provider.
    pub provider: String,
    /// Action type discriminator.
    pub action_type: String,
    /// JSON payload for the provider.
    pub payload: serde_json::Value,
    /// Cron expression (standard 5-field).
    pub cron_expression: String,
    /// Optional human-readable name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// IANA timezone for the cron expression. Defaults to UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Optional metadata labels.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub metadata: std::collections::HashMap<String, String>,
    /// Optional end date (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional dedup key template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub labels: std::collections::HashMap<String, String>,
}

/// Response from creating a recurring action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRecurringResponse {
    /// Assigned recurring action ID.
    pub id: String,
    /// Name (if provided).
    pub name: Option<String>,
    /// First scheduled execution time.
    pub next_execution_at: Option<String>,
    /// Status.
    pub status: String,
}

/// Filter parameters for listing recurring actions.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RecurringFilter {
    /// Namespace (required).
    pub namespace: String,
    /// Tenant (required).
    pub tenant: String,
    /// Optional status filter: "active" or "paused".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Maximum number of results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Number of results to skip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

/// Summary of a recurring action for list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringSummary {
    /// Unique recurring action ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Cron expression.
    pub cron_expr: String,
    /// IANA timezone.
    pub timezone: String,
    /// Whether the action is currently enabled.
    pub enabled: bool,
    /// Target provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// Next scheduled execution time.
    pub next_execution_at: Option<String>,
    /// Total execution count.
    pub execution_count: u64,
    /// Optional description.
    pub description: Option<String>,
    /// When the recurring action was created.
    pub created_at: String,
}

/// Response from listing recurring actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRecurringResponse {
    /// List of recurring action summaries.
    pub recurring_actions: Vec<RecurringSummary>,
    /// Total count of results returned.
    pub count: usize,
}

/// Full detail response for a single recurring action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringDetail {
    /// Unique recurring action ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Cron expression.
    pub cron_expr: String,
    /// IANA timezone.
    pub timezone: String,
    /// Whether the action is currently enabled.
    pub enabled: bool,
    /// Target provider.
    pub provider: String,
    /// Action type.
    pub action_type: String,
    /// JSON payload template.
    pub payload: serde_json::Value,
    /// Metadata labels.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// Optional dedup key template.
    pub dedup_key: Option<String>,
    /// Next scheduled execution time.
    pub next_execution_at: Option<String>,
    /// Most recent execution time.
    pub last_executed_at: Option<String>,
    /// Optional end date.
    pub ends_at: Option<String>,
    /// Total execution count.
    pub execution_count: u64,
    /// Optional description.
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
    /// When the recurring action was created.
    pub created_at: String,
    /// When the recurring action was last updated.
    pub updated_at: String,
}

/// Request to update a recurring action.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateRecurringAction {
    /// Namespace (required for key lookup).
    pub namespace: String,
    /// Tenant (required for key lookup).
    pub tenant: String,
    /// Updated name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Updated payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Updated metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
    /// Updated cron expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_expression: Option<String>,
    /// Updated timezone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Updated end date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated dedup key template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

impl ActeonClient {
    /// Create a new recurring action.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, CreateRecurringAction};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let req = CreateRecurringAction {
    ///     namespace: "notifications".to_string(),
    ///     tenant: "tenant-1".to_string(),
    ///     provider: "email".to_string(),
    ///     action_type: "send_digest".to_string(),
    ///     payload: serde_json::json!({"to": "user@example.com"}),
    ///     cron_expression: "0 9 * * MON-FRI".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let result = client.create_recurring(&req).await?;
    /// println!("Created recurring action: {}", result.id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_recurring(
        &self,
        req: &CreateRecurringAction,
    ) -> Result<CreateRecurringResponse, Error> {
        let url = format!("{}/v1/recurring", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<CreateRecurringResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
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

    /// List recurring actions filtered by namespace, tenant, and optional status.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, RecurringFilter};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let filter = RecurringFilter {
    ///     namespace: "notifications".to_string(),
    ///     tenant: "tenant-1".to_string(),
    ///     ..Default::default()
    /// };
    ///
    /// let result = client.list_recurring(&filter).await?;
    /// for action in result.recurring_actions {
    ///     println!("{}: {} ({})", action.id, action.cron_expr, action.provider);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_recurring(
        &self,
        filter: &RecurringFilter,
    ) -> Result<ListRecurringResponse, Error> {
        let url = format!("{}/v1/recurring", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListRecurringResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list recurring actions".to_string(),
            })
        }
    }

    /// Get the full details of a recurring action by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<RecurringDetail>, Error> {
        let url = format!("{}/v1/recurring/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RecurringDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to get recurring action".to_string(),
            })
        }
    }

    /// Update a recurring action.
    pub async fn update_recurring(
        &self,
        id: &str,
        update: &UpdateRecurringAction,
    ) -> Result<RecurringDetail, Error> {
        let url = format!("{}/v1/recurring/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RecurringDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Recurring action not found: {id}"),
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

    /// Delete a recurring action.
    pub async fn delete_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<(), Error> {
        let url = format!("{}/v1/recurring/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.delete(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Recurring action not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete recurring action".to_string(),
            })
        }
    }

    /// Pause a recurring action, removing it from the schedule.
    pub async fn pause_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<RecurringDetail, Error> {
        let url = format!("{}/v1/recurring/{}/pause", self.base_url, id);

        let response = self
            .add_auth(self.client.post(&url))
            .json(&serde_json::json!({
                "namespace": namespace,
                "tenant": tenant,
            }))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RecurringDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Recurring action not found: {id}"),
            })
        } else if response.status() == reqwest::StatusCode::CONFLICT {
            Err(Error::Http {
                status: 409,
                message: "Recurring action is already paused".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to pause recurring action".to_string(),
            })
        }
    }

    /// Resume a paused recurring action.
    pub async fn resume_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<RecurringDetail, Error> {
        let url = format!("{}/v1/recurring/{}/resume", self.base_url, id);

        let response = self
            .add_auth(self.client.post(&url))
            .json(&serde_json::json!({
                "namespace": namespace,
                "tenant": tenant,
            }))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RecurringDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Recurring action not found: {id}"),
            })
        } else if response.status() == reqwest::StatusCode::CONFLICT {
            Err(Error::Http {
                status: 409,
                message: "Recurring action is already active".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to resume recurring action".to_string(),
            })
        }
    }
}
