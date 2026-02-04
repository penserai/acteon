//! Acteon HTTP Client
//!
//! A native Rust client for interacting with the Acteon action gateway via its REST API.
//!
//! # Quick Start
//!
//! ```no_run
//! use acteon_client::{ActeonClient, ActeonClientBuilder};
//! use acteon_core::Action;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), acteon_client::Error> {
//!     // Create a client
//!     let client = ActeonClient::new("http://localhost:8080");
//!
//!     // Check health
//!     if client.health().await? {
//!         println!("Server is healthy");
//!     }
//!
//!     // Dispatch an action
//!     let action = Action::new(
//!         "notifications",
//!         "tenant-1",
//!         "email",
//!         "send_notification",
//!         serde_json::json!({"to": "user@example.com", "subject": "Hello"}),
//!     );
//!
//!     let outcome = client.dispatch(&action).await?;
//!     println!("Outcome: {:?}", outcome);
//!
//!     Ok(())
//! }
//! ```
//!
//! # Features
//!
//! - Single and batch action dispatch
//! - Rule management (list, reload, enable/disable)
//! - Audit trail queries
//! - Configurable timeouts and retry policies
//! - Builder pattern for advanced configuration
//!
//! # Configuration
//!
//! Use the builder pattern for custom configuration:
//!
//! ```no_run
//! use acteon_client::ActeonClientBuilder;
//! use std::time::Duration;
//!
//! let client = ActeonClientBuilder::new("http://localhost:8080")
//!     .timeout(Duration::from_secs(30))
//!     .api_key("your-api-key")
//!     .build()
//!     .unwrap();
//! ```

mod error;

pub use error::Error;

use acteon_core::{Action, ActionOutcome};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for the Acteon action gateway.
///
/// Provides methods for dispatching actions, managing rules, and querying audit logs
/// via the REST API.
#[derive(Debug, Clone)]
pub struct ActeonClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

/// Builder for configuring an [`ActeonClient`].
#[derive(Debug)]
pub struct ActeonClientBuilder {
    base_url: String,
    timeout: Duration,
    api_key: Option<String>,
    client: Option<Client>,
}

impl ActeonClientBuilder {
    /// Create a new builder with the given base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: DEFAULT_TIMEOUT,
            api_key: None,
            client: None,
        }
    }

    /// Set the request timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the API key for authentication.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Use a custom reqwest Client.
    ///
    /// Useful for configuring TLS, proxies, or other advanced settings.
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<ActeonClient, Error> {
        let client = match self.client {
            Some(c) => c,
            None => Client::builder()
                .timeout(self.timeout)
                .build()
                .map_err(|e| Error::Configuration(e.to_string()))?,
        };

        Ok(ActeonClient {
            client,
            base_url: self.base_url,
            api_key: self.api_key,
        })
    }
}

impl ActeonClient {
    /// Create a new client with default configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// ```
    pub fn new(base_url: impl Into<String>) -> Self {
        ActeonClientBuilder::new(base_url)
            .build()
            .expect("default client configuration should not fail")
    }

    /// Create a builder for advanced configuration.
    pub fn builder(base_url: impl Into<String>) -> ActeonClientBuilder {
        ActeonClientBuilder::new(base_url)
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Add authorization header if API key is set.
    fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => req.header("Authorization", format!("Bearer {key}")),
            None => req,
        }
    }

    // =========================================================================
    // Health
    // =========================================================================

    /// Check if the server is healthy.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let healthy = client.health().await?;
    /// assert!(healthy);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn health(&self) -> Result<bool, Error> {
        let url = format!("{}/health", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        Ok(response.status().is_success())
    }

    // =========================================================================
    // Action Dispatch
    // =========================================================================

    /// Dispatch a single action.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));
    ///
    /// let outcome = client.dispatch(&action).await?;
    /// println!("Outcome: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch(&self, action: &Action) -> Result<ActionOutcome, Error> {
        let url = format!("{}/v1/dispatch", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(action)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let outcome = response
                .json::<ActionOutcome>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(outcome)
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

    /// Dispatch multiple actions in a single request.
    ///
    /// Returns a result for each action, preserving order.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let actions = vec![
    ///     Action::new("ns", "tenant", "email", "send", serde_json::json!({})),
    ///     Action::new("ns", "tenant", "sms", "send", serde_json::json!({})),
    /// ];
    ///
    /// let results = client.dispatch_batch(&actions).await?;
    /// for result in results {
    ///     match result {
    ///         acteon_client::BatchResult::Success(outcome) => println!("Success: {:?}", outcome),
    ///         acteon_client::BatchResult::Error { error } => println!("Error: {}", error.message),
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_batch(&self, actions: &[Action]) -> Result<Vec<BatchResult>, Error> {
        let url = format!("{}/v1/dispatch/batch", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(actions)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let results = response
                .json::<Vec<BatchResult>>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(results)
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

    // =========================================================================
    // Rules Management
    // =========================================================================

    /// List all loaded rules.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let rules = client.list_rules().await?;
    /// for rule in rules {
    ///     println!("{}: priority={}, enabled={}", rule.name, rule.priority, rule.enabled);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_rules(&self) -> Result<Vec<RuleInfo>, Error> {
        let url = format!("{}/v1/rules", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let rules = response
                .json::<Vec<RuleInfo>>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(rules)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list rules: {}", response.status()),
            })
        }
    }

    /// Reload rules from the configured directory.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.reload_rules().await?;
    /// println!("Loaded {} rules", result.loaded);
    /// if !result.errors.is_empty() {
    ///     println!("Errors: {:?}", result.errors);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reload_rules(&self) -> Result<ReloadResult, Error> {
        let url = format!("{}/v1/rules/reload", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ReloadResult>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to reload rules: {}", response.status()),
            })
        }
    }

    /// Enable or disable a specific rule.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// client.set_rule_enabled("block-spam", false).await?;
    /// println!("Rule disabled");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_rule_enabled(&self, rule_name: &str, enabled: bool) -> Result<(), Error> {
        let url = format!("{}/v1/rules/{}/enabled", self.base_url, rule_name);

        let response = self
            .add_auth(self.client.put(&url))
            .json(&serde_json::json!({ "enabled": enabled }))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to set rule enabled: {}", response.status()),
            })
        }
    }

    // =========================================================================
    // Audit Trail
    // =========================================================================

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
}

// =============================================================================
// Response Types
// =============================================================================

/// Error response from the API.
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    /// Error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether the request can be retried.
    #[serde(default)]
    pub retryable: bool,
}

/// Result from a batch dispatch operation.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BatchResult {
    /// Action was processed successfully.
    Success(ActionOutcome),
    /// Action processing failed.
    Error {
        /// Error details.
        error: ErrorResponse,
    },
}

impl BatchResult {
    /// Returns `true` if this is a success result.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    /// Returns `true` if this is an error result.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Returns the outcome if this is a success result.
    pub fn outcome(&self) -> Option<&ActionOutcome> {
        match self {
            Self::Success(outcome) => Some(outcome),
            Self::Error { .. } => None,
        }
    }

    /// Returns the error if this is an error result.
    pub fn error(&self) -> Option<&ErrorResponse> {
        match self {
            Self::Success(_) => None,
            Self::Error { error } => Some(error),
        }
    }
}

/// Information about a loaded rule.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleInfo {
    /// Rule name.
    pub name: String,
    /// Rule priority (lower = higher priority).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Optional rule description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Result of reloading rules.
#[derive(Debug, Clone, Deserialize)]
pub struct ReloadResult {
    /// Number of rules loaded.
    pub loaded: usize,
    /// Any errors that occurred during loading.
    pub errors: Vec<String>,
}

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
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_trims_trailing_slash() {
        let client = ActeonClient::new("http://localhost:8080/");
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[test]
    fn client_preserves_url_without_slash() {
        let client = ActeonClient::new("http://localhost:8080");
        assert_eq!(client.base_url(), "http://localhost:8080");
    }

    #[test]
    fn builder_sets_api_key() {
        let client = ActeonClientBuilder::new("http://localhost:8080")
            .api_key("test-key")
            .build()
            .unwrap();
        assert_eq!(client.api_key, Some("test-key".to_string()));
    }

    #[test]
    fn batch_result_helpers() {
        use acteon_core::ProviderResponse;

        let success = BatchResult::Success(ActionOutcome::Executed(
            ProviderResponse::success(serde_json::json!({})),
        ));
        assert!(success.is_success());
        assert!(!success.is_error());
        assert!(success.outcome().is_some());
        assert!(success.error().is_none());

        let error = BatchResult::Error {
            error: ErrorResponse {
                code: "ERR".to_string(),
                message: "test".to_string(),
                retryable: false,
            },
        };
        assert!(!error.is_success());
        assert!(error.is_error());
        assert!(error.outcome().is_none());
        assert!(error.error().is_some());
    }
}
