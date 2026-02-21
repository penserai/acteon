use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Request to create a data retention policy.
#[derive(Debug, Clone, Serialize)]
pub struct CreateRetentionRequest {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Override for the global audit TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_ttl_seconds: Option<u64>,
    /// TTL for completed chain state records (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_ttl_seconds: Option<u64>,
    /// TTL for resolved event state records (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ttl_seconds: Option<u64>,
    /// When `true`, audit records never expire (compliance hold).
    #[serde(default)]
    pub compliance_hold: bool,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update a data retention policy.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateRetentionRequest {
    /// Updated enabled state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Updated audit TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_ttl_seconds: Option<u64>,
    /// Updated state TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_ttl_seconds: Option<u64>,
    /// Updated event TTL (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ttl_seconds: Option<u64>,
    /// Updated compliance hold flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compliance_hold: Option<bool>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// A data retention policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Unique policy ID.
    pub id: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Whether this policy is active.
    pub enabled: bool,
    /// Audit TTL override (seconds).
    #[serde(default)]
    pub audit_ttl_seconds: Option<u64>,
    /// State TTL (seconds).
    #[serde(default)]
    pub state_ttl_seconds: Option<u64>,
    /// Event TTL (seconds).
    #[serde(default)]
    pub event_ttl_seconds: Option<u64>,
    /// Compliance hold flag.
    #[serde(default)]
    pub compliance_hold: bool,
    /// When the policy was created.
    pub created_at: String,
    /// When the policy was last updated.
    pub updated_at: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Response from listing retention policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRetentionResponse {
    /// List of retention policies.
    pub policies: Vec<RetentionPolicy>,
    /// Total count of results.
    pub count: usize,
}

impl ActeonClient {
    /// Create a new retention policy.
    pub async fn create_retention(
        &self,
        req: &CreateRetentionRequest,
    ) -> Result<RetentionPolicy, Error> {
        let url = format!("{}/v1/retention", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RetentionPolicy>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to create retention policy".to_string(),
            })
        }
    }

    /// List retention policies, optionally filtered by namespace and tenant.
    pub async fn list_retention(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListRetentionResponse, Error> {
        let url = format!("{}/v1/retention", self.base_url);

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
                .json::<ListRetentionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list retention policies".to_string(),
            })
        }
    }

    /// Get a single retention policy by ID.
    pub async fn get_retention(&self, id: &str) -> Result<Option<RetentionPolicy>, Error> {
        let url = format!("{}/v1/retention/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RetentionPolicy>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get retention policy: {id}"),
            })
        }
    }

    /// Update a retention policy.
    pub async fn update_retention(
        &self,
        id: &str,
        update: &UpdateRetentionRequest,
    ) -> Result<RetentionPolicy, Error> {
        let url = format!("{}/v1/retention/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RetentionPolicy>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Retention policy not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to update retention policy".to_string(),
            })
        }
    }

    /// Delete a retention policy.
    pub async fn delete_retention(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/retention/{}", self.base_url, id);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Retention policy not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete retention policy".to_string(),
            })
        }
    }
}
