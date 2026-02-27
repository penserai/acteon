use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Request to create a quota policy.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CreateQuotaRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Maximum number of actions allowed in the window.
    pub max_actions: u64,
    /// Time window (e.g., "1h", "24h", "7d").
    pub window: String,
    /// Behavior when quota is exceeded: "reject" or "warn".
    pub overage_behavior: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update a quota policy.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UpdateQuotaRequest {
    /// Namespace (required for key lookup).
    pub namespace: String,
    /// Tenant (required for key lookup).
    pub tenant: String,
    /// Updated maximum actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_actions: Option<u64>,
    /// Updated time window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Updated overage behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overage_behavior: Option<String>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the quota is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// A quota policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaPolicy {
    /// Unique quota policy ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Maximum number of actions allowed in the window.
    pub max_actions: u64,
    /// Time window (e.g., "1h", "24h", "7d").
    pub window: String,
    /// Behavior when quota is exceeded.
    pub overage_behavior: String,
    /// Whether the quota is currently enabled.
    pub enabled: bool,
    /// When the quota was created.
    pub created_at: String,
    /// When the quota was last updated.
    pub updated_at: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Response from listing quota policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListQuotasResponse {
    /// List of quota policies.
    pub quotas: Vec<QuotaPolicy>,
    /// Total count of results.
    pub count: usize,
}

/// Current usage statistics for a quota.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsage {
    /// Tenant.
    pub tenant: String,
    /// Namespace.
    pub namespace: String,
    /// Number of actions used in the current window.
    pub used: u64,
    /// The quota limit.
    pub limit: u64,
    /// Remaining actions in the current window.
    pub remaining: u64,
    /// Time window.
    pub window: String,
    /// When the current window resets (ISO 8601).
    pub resets_at: String,
    /// Overage behavior.
    pub overage_behavior: String,
}

impl ActeonClient {
    /// Create a new quota policy.
    pub async fn create_quota(&self, req: &CreateQuotaRequest) -> Result<QuotaPolicy, Error> {
        let url = format!("{}/v1/quotas", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<QuotaPolicy>()
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

    /// List quota policies filtered by optional namespace and tenant.
    pub async fn list_quotas(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListQuotasResponse, Error> {
        let url = format!("{}/v1/quotas", self.base_url);

        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(ns) = namespace {
            query.push(("namespace", ns));
        }
        if let Some(t) = tenant {
            query.push(("tenant", t));
        }

        let response = self
            .add_auth(self.client.get(&url))
            .query(&query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListQuotasResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list quotas".to_string(),
            })
        }
    }

    /// Get a single quota policy by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_quota(&self, id: &str) -> Result<Option<QuotaPolicy>, Error> {
        let url = format!("{}/v1/quotas/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<QuotaPolicy>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to get quota".to_string(),
            })
        }
    }

    /// Update a quota policy.
    pub async fn update_quota(
        &self,
        id: &str,
        update: &UpdateQuotaRequest,
    ) -> Result<QuotaPolicy, Error> {
        let url = format!("{}/v1/quotas/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<QuotaPolicy>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Quota not found: {id}"),
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

    /// Delete a quota policy.
    pub async fn delete_quota(&self, id: &str, namespace: &str, tenant: &str) -> Result<(), Error> {
        let url = format!("{}/v1/quotas/{}", self.base_url, id);

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
                message: format!("Quota not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete quota".to_string(),
            })
        }
    }

    /// Get current usage statistics for a quota policy.
    pub async fn get_quota_usage(&self, id: &str) -> Result<QuotaUsage, Error> {
        let url = format!("{}/v1/quotas/{}/usage", self.base_url, id);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<QuotaUsage>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Quota not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to get quota usage".to_string(),
            })
        }
    }
}
