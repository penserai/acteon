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

pub mod aws;
mod error;
pub mod stream;
pub mod webhook;

pub use error::Error;
pub use stream::{EventStream, StreamFilter, StreamItem};

// Re-export core attachment type so callers don't need a direct `acteon_core` dependency.
pub use acteon_core::Attachment;

use std::fmt::Write;
use std::time::Duration;

use acteon_core::{
    Action, ActionOutcome, CircuitBreakerActionResponse, ListCircuitBreakersResponse,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};

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
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the API key for authentication.
    #[must_use]
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Use a custom reqwest Client.
    ///
    /// Useful for configuring TLS, proxies, or other advanced settings.
    #[must_use]
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
        self.dispatch_inner(action, false).await
    }

    /// Dispatch a single action in dry-run mode.
    ///
    /// Evaluates rules and returns the verdict without executing the action,
    /// recording state, or emitting audit records.
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
    /// let outcome = client.dispatch_dry_run(&action).await?;
    /// println!("Would result in: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_dry_run(&self, action: &Action) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, true).await
    }

    /// Dispatch a single action with file attachments.
    ///
    /// This is a convenience wrapper that clones the action, sets the given
    /// attachments, and dispatches it. For repeated use, prefer constructing
    /// the action with [`Action::with_attachments`] directly.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, Attachment};
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));
    ///
    /// let attachments = vec![
    ///     Attachment {
    ///         id: "att-1".into(),
    ///         name: "Hello".into(),
    ///         filename: "hello.txt".into(),
    ///         content_type: "text/plain".into(),
    ///         data_base64: "SGVsbG8=".into(),
    ///     },
    /// ];
    ///
    /// let outcome = client.dispatch_with_attachments(&action, attachments).await?;
    /// println!("Outcome: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_with_attachments(
        &self,
        action: &Action,
        attachments: Vec<Attachment>,
    ) -> Result<ActionOutcome, Error> {
        let mut action = action.clone();
        action.attachments = attachments;
        self.dispatch_inner(&action, false).await
    }

    async fn dispatch_inner(&self, action: &Action, dry_run: bool) -> Result<ActionOutcome, Error> {
        let mut url = format!("{}/v1/dispatch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

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
        self.dispatch_batch_inner(actions, false).await
    }

    /// Dispatch multiple actions in dry-run mode.
    ///
    /// Evaluates rules for each action and returns the verdicts without
    /// executing any actions.
    pub async fn dispatch_batch_dry_run(
        &self,
        actions: &[Action],
    ) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, true).await
    }

    async fn dispatch_batch_inner(
        &self,
        actions: &[Action],
        dry_run: bool,
    ) -> Result<Vec<BatchResult>, Error> {
        let mut url = format!("{}/v1/dispatch/batch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

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

    /// Evaluate rules against a test action without dispatching.
    ///
    /// Returns a detailed trace showing how each rule would evaluate against
    /// the given action. This is useful for debugging and testing rule
    /// configurations in a playground environment.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, EvaluateRulesOptions};
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new(
    ///     "notifications",
    ///     "tenant-1",
    ///     "email",
    ///     "send_notification",
    ///     serde_json::json!({"to": "user@example.com"}),
    /// );
    ///
    /// let options = EvaluateRulesOptions {
    ///     evaluate_all: true,
    ///     ..Default::default()
    /// };
    ///
    /// let trace = client.evaluate_rules(&action, &options).await?;
    /// println!("Verdict: {}", trace.verdict);
    /// for entry in &trace.trace {
    ///     println!("  {} -> {}", entry.rule_name, entry.result);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn evaluate_rules(
        &self,
        action: &Action,
        options: &EvaluateRulesOptions,
    ) -> Result<RuleEvaluationTrace, Error> {
        #[derive(Serialize)]
        struct Body<'a> {
            namespace: &'a str,
            tenant: &'a str,
            provider: &'a str,
            action_type: &'a str,
            payload: &'a serde_json::Value,
            metadata: &'a std::collections::HashMap<String, String>,
            #[serde(flatten)]
            options: &'a EvaluateRulesOptions,
        }

        let url = format!("{}/v1/rules/evaluate", self.base_url);

        let body = Body {
            namespace: action.namespace.as_str(),
            tenant: action.tenant.as_str(),
            provider: action.provider.as_str(),
            action_type: &action.action_type,
            payload: &action.payload,
            metadata: &action.metadata.labels,
            options,
        };

        let response = self
            .add_auth(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let trace = response
                .json::<RuleEvaluationTrace>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(trace)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to evaluate rules: {}", response.status()),
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

    // =========================================================================
    // Audit Replay
    // =========================================================================

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

    // =========================================================================
    // Events (State Machine Lifecycle)
    // =========================================================================

    /// List events filtered by namespace, tenant, and optionally status.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, EventQuery};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = EventQuery {
    ///     namespace: "notifications".to_string(),
    ///     tenant: "tenant-1".to_string(),
    ///     status: Some("open".to_string()),
    ///     limit: Some(50),
    /// };
    /// let events = client.list_events(&query).await?;
    /// for event in events.events {
    ///     println!("{}: {}", event.fingerprint, event.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_events(&self, query: &EventQuery) -> Result<EventListResponse, Error> {
        let url = format!("{}/v1/events", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<EventListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list events: {}", response.status()),
            })
        }
    }

    /// Get the current state of an event by fingerprint.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(event) = client.get_event("fingerprint-123", "notifications", "tenant-1").await? {
    ///     println!("Event state: {}", event.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_event(
        &self,
        fingerprint: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<EventState>, Error> {
        let url = format!("{}/v1/events/{}", self.base_url, fingerprint);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let event = response
                .json::<EventState>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(event))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get event: {}", response.status()),
            })
        }
    }

    /// Transition an event to a new state.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.transition_event(
    ///     "fingerprint-123",
    ///     "investigating",
    ///     "notifications",
    ///     "tenant-1"
    /// ).await?;
    /// println!("Transitioned from {} to {}", result.previous_state, result.new_state);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transition_event(
        &self,
        fingerprint: &str,
        to_state: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<TransitionResponse, Error> {
        let url = format!("{}/v1/events/{}/transition", self.base_url, fingerprint);

        let body = serde_json::json!({
            "to": to_state,
            "namespace": namespace,
            "tenant": tenant,
        });

        let response = self
            .add_auth(self.client.put(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TransitionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Event not found: {fingerprint}"),
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

    // =========================================================================
    // Approvals (Human-in-the-Loop)
    // =========================================================================

    /// Approve a pending action by namespace, tenant, ID, and HMAC signature.
    ///
    /// The original action is executed upon approval. This does not require
    /// authentication -- the HMAC signature in the query string serves as
    /// proof of authorization.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.approve("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await?;
    /// println!("Status: {}, Outcome: {:?}", result.status, result.outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn approve(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<ApprovalActionResponse, Error> {
        self.approve_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Approve a pending action, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn approve_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<ApprovalActionResponse, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}/approve?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: "Approval not found or expired".to_string(),
            })
        } else if response.status() == reqwest::StatusCode::GONE {
            Err(Error::Http {
                status: 410,
                message: "Approval already decided".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to approve: {}", response.status()),
            })
        }
    }

    /// Reject a pending action by namespace, tenant, ID, and HMAC signature.
    ///
    /// This does not require authentication -- the HMAC signature in the
    /// query string serves as proof of authorization.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.reject("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await?;
    /// println!("Status: {}", result.status);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reject(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<ApprovalActionResponse, Error> {
        self.reject_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Reject a pending action, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn reject_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<ApprovalActionResponse, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}/reject?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: "Approval not found or expired".to_string(),
            })
        } else if response.status() == reqwest::StatusCode::GONE {
            Err(Error::Http {
                status: 410,
                message: "Approval already decided".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to reject: {}", response.status()),
            })
        }
    }

    /// Get the status of an approval by namespace, tenant, ID, and HMAC signature.
    ///
    /// Returns `None` if the approval is not found or has expired.
    /// Does not expose the original action payload.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(status) = client.get_approval("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await? {
    ///     println!("Approval status: {}", status.status);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_approval(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<Option<ApprovalStatusResponse>, Error> {
        self.get_approval_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Get approval status, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn get_approval_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<Option<ApprovalStatusResponse>, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalStatusResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get approval: {}", response.status()),
            })
        }
    }

    /// List pending approvals filtered by namespace and tenant.
    ///
    /// Requires authentication.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.list_approvals("payments", "tenant-1").await?;
    /// for approval in result.approvals {
    ///     println!("{}: {}", approval.token, approval.status);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_approvals(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<ApprovalListResponse, Error> {
        let url = format!("{}/v1/approvals", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list approvals: {}", response.status()),
            })
        }
    }

    // =========================================================================
    // Groups (Event Batching)
    // =========================================================================

    /// List all active event groups.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let groups = client.list_groups().await?;
    /// println!("Active groups: {}", groups.total);
    /// for group in groups.groups {
    ///     println!("{}: {} events", group.group_id, group.event_count);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_groups(&self) -> Result<GroupListResponse, Error> {
        let url = format!("{}/v1/groups", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<GroupListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list groups: {}", response.status()),
            })
        }
    }

    /// Get details of a specific group.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(group) = client.get_group("group-key-123").await? {
    ///     println!("Group {} has {} events", group.group.group_id, group.group.event_count);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_group(&self, group_key: &str) -> Result<Option<GroupDetail>, Error> {
        let url = format!("{}/v1/groups/{}", self.base_url, group_key);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<GroupDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get group: {}", response.status()),
            })
        }
    }

    /// Force flush a group, triggering immediate notification.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.flush_group("group-key-123").await?;
    /// println!("Flushed group {} with {} events", result.group_id, result.event_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn flush_group(&self, group_key: &str) -> Result<FlushGroupResponse, Error> {
        let url = format!("{}/v1/groups/{}", self.base_url, group_key);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<FlushGroupResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Group not found: {group_key}"),
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

    // =========================================================================
    // SSE Event Stream
    // =========================================================================

    /// Subscribe to the real-time SSE event stream.
    ///
    /// Returns an [`EventStream`] that yields [`StreamItem`]s as the server
    /// emits them. Use a [`StreamFilter`] to limit which events are received.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, StreamFilter};
    /// use futures::StreamExt;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let filter = StreamFilter::new().namespace("notifications");
    ///
    /// let mut stream = client.stream(&filter).await?;
    /// while let Some(item) = stream.next().await {
    ///     match item? {
    ///         acteon_client::StreamItem::Event(event) => {
    ///             println!("Event: {:?}", event);
    ///         }
    ///         acteon_client::StreamItem::Lagged { skipped } => {
    ///             eprintln!("Missed {skipped} events");
    ///         }
    ///         acteon_client::StreamItem::KeepAlive => {}
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(&self, filter: &StreamFilter) -> Result<EventStream, Error> {
        let url = format!("{}/v1/stream", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(stream::event_stream_from_response(response))
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            Err(Error::Http { status, message })
        }
    }

    /// Subscribe to events for a specific entity via the
    /// `GET /v1/subscribe/{entity_type}/{entity_id}` endpoint.
    ///
    /// This is a convenience method that opens an SSE stream pre-filtered
    /// for the given entity type and ID.
    pub async fn subscribe_entity(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<EventStream, Error> {
        let url = format!(
            "{}/v1/subscribe/{}/{}",
            self.base_url, entity_type, entity_id
        );

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(stream::event_stream_from_response(response))
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            Err(Error::Http { status, message })
        }
    }

    /// Subscribe to events for a specific chain.
    pub async fn subscribe_chain(&self, chain_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("chain", chain_id).await
    }

    /// Subscribe to events for a specific group.
    pub async fn subscribe_group(&self, group_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("group", group_id).await
    }

    /// Subscribe to events for a specific action.
    pub async fn subscribe_action(&self, action_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("action", action_id).await
    }

    // =========================================================================
    // Recurring Actions
    // =========================================================================

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

    // =========================================================================
    // Quotas
    // =========================================================================

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

    // =========================================================================
    // Retention Policies
    // =========================================================================

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

    // =========================================================================
    // Compliance (SOC2/HIPAA)
    // =========================================================================

    /// Get the current compliance configuration status.
    pub async fn get_compliance_status(&self) -> Result<ComplianceStatus, Error> {
        let url = format!("{}/v1/compliance/status", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ComplianceStatus>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to get compliance status".into(),
            })
        }
    }

    /// Verify the integrity of the audit hash chain for a namespace/tenant pair.
    pub async fn verify_audit_chain(
        &self,
        req: &VerifyHashChainRequest,
    ) -> Result<HashChainVerification, Error> {
        let url = format!("{}/v1/audit/verify", self.base_url);
        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<HashChainVerification>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to verify audit chain".into(),
            })
        }
    }

    // =========================================================================
    // Chains
    // =========================================================================

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

    // =========================================================================
    // Dead Letter Queue (DLQ)
    // =========================================================================

    /// Get dead letter queue statistics.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let stats = client.dlq_stats().await?;
    /// println!("DLQ enabled: {}, count: {}", stats.enabled, stats.count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dlq_stats(&self) -> Result<DlqStatsResponse, Error> {
        let url = format!("{}/v1/dlq/stats", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DlqStatsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get DLQ stats: {}", response.status()),
            })
        }
    }

    /// Drain all entries from the dead letter queue.
    ///
    /// Returns the drained entries along with a count.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.dlq_drain().await?;
    /// println!("Drained {} entries", result.count);
    /// for entry in &result.entries {
    ///     println!("  {}: {} ({})", entry.action_id, entry.error, entry.attempts);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dlq_drain(&self) -> Result<DlqDrainResponse, Error> {
        let url = format!("{}/v1/dlq/drain", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DlqDrainResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to drain DLQ: {}", response.status()),
            })
        }
    }

    // =========================================================================
    // Circuit Breakers
    // =========================================================================

    /// List all circuit breakers and their current status.
    ///
    /// Requires admin permissions.
    pub async fn list_circuit_breakers(&self) -> Result<ListCircuitBreakersResponse, Error> {
        let url = format!("{}/admin/circuit-breakers", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListCircuitBreakersResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list circuit breakers: {}", response.status()),
            })
        }
    }

    /// Trip (force open) a circuit breaker for a specific provider.
    ///
    /// Requires admin permissions.
    pub async fn trip_circuit_breaker(
        &self,
        provider: &str,
    ) -> Result<CircuitBreakerActionResponse, Error> {
        let url = format!("{}/admin/circuit-breakers/{}/trip", self.base_url, provider);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<CircuitBreakerActionResponse>()
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

    /// Reset (force close) a circuit breaker for a specific provider.
    ///
    /// Requires admin permissions.
    pub async fn reset_circuit_breaker(
        &self,
        provider: &str,
    ) -> Result<CircuitBreakerActionResponse, Error> {
        let url = format!(
            "{}/admin/circuit-breakers/{}/reset",
            self.base_url, provider
        );

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<CircuitBreakerActionResponse>()
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

    // =========================================================================
    // Provider Health
    // =========================================================================

    /// List per-provider health status, execution metrics, and latency percentiles.
    pub async fn list_provider_health(
        &self,
    ) -> Result<acteon_core::ListProviderHealthResponse, Error> {
        let url = format!("{}/v1/providers/health", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<acteon_core::ListProviderHealthResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list provider health: {}", response.status()),
            })
        }
    }

    // =========================================================================
    // WASM Plugins
    // =========================================================================

    /// List all registered WASM plugins.
    pub async fn list_plugins(&self) -> Result<ListPluginsResponse, Error> {
        let url = format!("{}/v1/plugins", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListPluginsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list plugins".to_string(),
            })
        }
    }

    /// Register a new WASM plugin.
    pub async fn register_plugin(&self, req: &RegisterPluginRequest) -> Result<WasmPlugin, Error> {
        let url = format!("{}/v1/plugins", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<WasmPlugin>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to register plugin".to_string(),
            })
        }
    }

    /// Get details of a registered WASM plugin by name.
    pub async fn get_plugin(&self, name: &str) -> Result<Option<WasmPlugin>, Error> {
        let url = format!("{}/v1/plugins/{name}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<WasmPlugin>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get plugin: {name}"),
            })
        }
    }

    /// Unregister (delete) a WASM plugin by name.
    pub async fn delete_plugin(&self, name: &str) -> Result<(), Error> {
        let url = format!("{}/v1/plugins/{name}", self.base_url);

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
                message: format!("Plugin not found: {name}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete plugin".to_string(),
            })
        }
    }

    /// Test-invoke a WASM plugin with a sample action context.
    pub async fn invoke_plugin(
        &self,
        name: &str,
        req: &PluginInvocationRequest,
    ) -> Result<PluginInvocationResponse, Error> {
        let url = format!("{}/v1/plugins/{name}/invoke", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<PluginInvocationResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Plugin not found: {name}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to invoke plugin: {name}"),
            })
        }
    }

    // =========================================================================
    // Templates
    // =========================================================================

    /// Create a new template.
    pub async fn create_template(
        &self,
        req: &CreateTemplateRequest,
    ) -> Result<TemplateInfo, Error> {
        let url = format!("{}/v1/templates", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
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

    /// List templates, optionally filtered by namespace and tenant.
    pub async fn list_templates(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListTemplatesResponse, Error> {
        let url = format!("{}/v1/templates", self.base_url);

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
                .json::<ListTemplatesResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list templates".to_string(),
            })
        }
    }

    /// Get a single template by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_template(&self, id: &str) -> Result<Option<TemplateInfo>, Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get template: {id}"),
            })
        }
    }

    /// Update a template.
    pub async fn update_template(
        &self,
        id: &str,
        update: &UpdateTemplateRequest,
    ) -> Result<TemplateInfo, Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template not found: {id}"),
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

    /// Delete a template.
    pub async fn delete_template(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/templates/{id}", self.base_url);

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
                message: format!("Template not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete template".to_string(),
            })
        }
    }

    // =========================================================================
    // Template Profiles
    // =========================================================================

    /// Create a new template profile.
    pub async fn create_profile(
        &self,
        req: &CreateProfileRequest,
    ) -> Result<TemplateProfileInfo, Error> {
        let url = format!("{}/v1/templates/profiles", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
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

    /// List template profiles, optionally filtered by namespace and tenant.
    pub async fn list_profiles(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListProfilesResponse, Error> {
        let url = format!("{}/v1/templates/profiles", self.base_url);

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
                .json::<ListProfilesResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list template profiles".to_string(),
            })
        }
    }

    /// Get a single template profile by ID.
    ///
    /// Returns `None` if not found.
    pub async fn get_profile(&self, id: &str) -> Result<Option<TemplateProfileInfo>, Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get template profile: {id}"),
            })
        }
    }

    /// Update a template profile.
    pub async fn update_profile(
        &self,
        id: &str,
        update: &UpdateProfileRequest,
    ) -> Result<TemplateProfileInfo, Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

        let response = self
            .add_auth(self.client.put(&url))
            .json(update)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TemplateProfileInfo>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Template profile not found: {id}"),
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

    /// Delete a template profile.
    pub async fn delete_profile(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/templates/profiles/{id}", self.base_url);

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
                message: format!("Template profile not found: {id}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete template profile".to_string(),
            })
        }
    }

    /// Render a template preview using the given profile and payload.
    pub async fn render_preview(
        &self,
        req: &RenderPreviewRequest,
    ) -> Result<RenderPreviewResponse, Error> {
        let url = format!("{}/v1/templates/render", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<RenderPreviewResponse>()
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
}

// =============================================================================
// Response Types
// =============================================================================

/// Error response from the API.
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadResult {
    /// Number of rules loaded.
    pub loaded: usize,
    /// Any errors that occurred during loading.
    pub errors: Vec<String>,
}

/// Options for rule evaluation playground requests.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvaluateRulesOptions {
    /// When `true`, includes disabled rules in the trace.
    #[serde(default)]
    pub include_disabled: bool,
    /// When `true`, evaluates every rule even after a match.
    #[serde(default)]
    pub evaluate_all: bool,
    /// Optional timestamp override for time-travel debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluate_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional state key overrides for testing state-dependent conditions.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub mock_state: std::collections::HashMap<String, String>,
}

/// Details about a semantic match evaluation, used for explainability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMatchDetail {
    /// The text that was extracted and compared.
    pub extracted_text: String,
    /// The topic the text was compared against.
    pub topic: String,
    /// The computed similarity score.
    pub similarity: f64,
    /// The threshold that was configured on the rule.
    pub threshold: f64,
}

/// Per-rule trace entry returned by the playground.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTraceEntry {
    /// Name of the rule that was evaluated.
    pub rule_name: String,
    /// Rule priority (lower = higher priority).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Human-readable display of the rule condition.
    pub condition_display: String,
    /// Evaluation result (e.g. `"matched"`, `"no_match"`, `"error"`).
    pub result: String,
    /// Time spent evaluating this rule in microseconds.
    pub evaluation_duration_us: u64,
    /// The action the rule would take on match.
    pub action: String,
    /// The source of the rule (e.g. `"yaml"`, `"cel"`).
    pub source: String,
    /// Optional description of the rule.
    pub description: Option<String>,
    /// Reason the rule was skipped, if applicable.
    pub skip_reason: Option<String>,
    /// Error message if evaluation failed.
    pub error: Option<String>,
    /// Details about semantic match evaluation, if the rule uses `SemanticMatch`.
    #[serde(default)]
    pub semantic_details: Option<SemanticMatchDetail>,
    /// The JSON merge patch this rule would apply (only for `Modify` rules in
    /// `evaluate_all` mode).
    #[serde(default)]
    pub modify_patch: Option<serde_json::Value>,
    /// Cumulative payload after applying this rule's patch (only for `Modify`
    /// rules in `evaluate_all` mode).
    #[serde(default)]
    pub modified_payload_preview: Option<serde_json::Value>,
}

/// Contextual information captured during rule evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// The `time.*` map that was used during evaluation.
    #[serde(default)]
    pub time: serde_json::Value,
    /// Environment keys that were actually accessed during evaluation
    /// (values omitted for security).
    #[serde(default)]
    pub environment_keys: Vec<String>,
    /// State keys that were actually accessed during evaluation.
    #[serde(default)]
    pub accessed_state_keys: Vec<String>,
    /// The effective timezone used for time-based conditions, if any.
    #[serde(default)]
    pub effective_timezone: Option<String>,
}

/// Response from the rule evaluation playground.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEvaluationTrace {
    /// Final verdict (e.g. `"allow"`, `"deny"`, `"no_match"`).
    pub verdict: String,
    /// Name of the matched rule, if any.
    pub matched_rule: Option<String>,
    /// Whether any rule produced an error during evaluation.
    #[serde(default)]
    pub has_errors: bool,
    /// Total number of rules that were evaluated.
    pub total_rules_evaluated: usize,
    /// Total number of rules that were skipped.
    pub total_rules_skipped: usize,
    /// Total evaluation time in microseconds.
    pub evaluation_duration_us: u64,
    /// Per-rule trace entries.
    pub trace: Vec<RuleTraceEntry>,
    /// The evaluation context that was used.
    pub context: TraceContext,
    /// The payload after any rule modifications, if changed.
    pub modified_payload: Option<serde_json::Value>,
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

// =============================================================================
// Replay Types
// =============================================================================

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

// =============================================================================
// Event Types (State Machine Lifecycle)
// =============================================================================

/// Query parameters for listing events.
#[derive(Debug, Default, Clone, Serialize)]
pub struct EventQuery {
    /// Filter by namespace (required).
    pub namespace: String,
    /// Filter by tenant (required).
    pub tenant: String,
    /// Filter by state (e.g., "open", "closed").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Maximum number of results to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Current state of an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventState {
    /// The event fingerprint.
    pub fingerprint: String,
    /// Current state of the event.
    pub state: String,
    /// The action type that created this event.
    pub action_type: Option<String>,
    /// When the state was last updated.
    pub updated_at: Option<String>,
}

/// Response from listing events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListResponse {
    /// List of events.
    pub events: Vec<EventState>,
    /// Total number of events returned.
    pub count: usize,
}

/// Response from transitioning an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionResponse {
    /// The event fingerprint.
    pub fingerprint: String,
    /// The previous state.
    pub previous_state: String,
    /// The new state.
    pub new_state: String,
    /// Whether the transition triggered a notification.
    pub notify: bool,
}

// =============================================================================
// Approval Types (Human-in-the-Loop)
// =============================================================================

/// Response from approving or rejecting an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalActionResponse {
    /// The approval ID.
    pub id: String,
    /// The resulting status ("approved" or "rejected").
    pub status: String,
    /// The outcome of the original action (only present when approved).
    pub outcome: Option<serde_json::Value>,
}

/// Public-facing approval status (no payload exposed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalStatusResponse {
    /// The approval token.
    pub token: String,
    /// Current status: "pending", "approved", or "rejected".
    pub status: String,
    /// Rule that triggered the approval.
    pub rule: String,
    /// When the approval was created.
    pub created_at: String,
    /// When the approval expires.
    pub expires_at: String,
    /// When a decision was made (if any).
    pub decided_at: Option<String>,
    /// Optional message from the rule.
    pub message: Option<String>,
}

/// Response from listing pending approvals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalListResponse {
    /// List of pending approvals.
    pub approvals: Vec<ApprovalStatusResponse>,
    /// Total number of approvals returned.
    pub count: usize,
}

// =============================================================================
// Group Types (Event Batching)
// =============================================================================

/// Summary of an event group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    /// Unique identifier for the group.
    pub group_id: String,
    /// The group key used for matching events.
    pub group_key: String,
    /// Number of events in the group.
    pub event_count: usize,
    /// Current state of the group.
    pub state: String,
    /// When the group will be notified.
    pub notify_at: Option<String>,
    /// When the group was created.
    pub created_at: String,
}

/// Response from listing groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupListResponse {
    /// List of groups.
    pub groups: Vec<GroupSummary>,
    /// Total number of groups.
    pub total: usize,
}

/// Detailed information about a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDetail {
    /// Group summary.
    pub group: GroupSummary,
    /// Event fingerprints in this group.
    pub events: Vec<String>,
    /// Labels used to group events.
    pub labels: std::collections::HashMap<String, String>,
}

/// Response from flushing a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushGroupResponse {
    /// The group ID that was flushed.
    pub group_id: String,
    /// Number of events that were flushed.
    pub event_count: usize,
    /// Whether notification was sent.
    pub notified: bool,
}

// =============================================================================
// Recurring Action Types
// =============================================================================

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

// =============================================================================
// Quota Types
// =============================================================================

/// Request to create a quota policy.
#[derive(Debug, Default, Clone, Serialize)]
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
#[derive(Debug, Default, Clone, Serialize)]
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

// =============================================================================
// Retention Types
// =============================================================================

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

// =============================================================================
// Template Types
// =============================================================================

/// A reusable `MiniJinja` template stored in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Unique template ID.
    pub id: String,
    /// Template name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this template belongs to.
    pub namespace: String,
    /// Tenant this template belongs to.
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this template was created.
    pub created_at: String,
    /// When this template was last updated.
    pub updated_at: String,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// A template profile that maps payload fields to template content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateProfileInfo {
    /// Unique profile ID.
    pub id: String,
    /// Profile name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this profile belongs to.
    pub namespace: String,
    /// Tenant this profile belongs to.
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// When this profile was created.
    pub created_at: String,
    /// When this profile was last updated.
    pub updated_at: String,
    /// Arbitrary key-value labels.
    #[serde(default)]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to create a new template.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTemplateRequest {
    /// Template name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Raw `MiniJinja` template content.
    pub content: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update an existing template.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateTemplateRequest {
    /// Updated template content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to create a new template profile.
#[derive(Debug, Clone, Serialize)]
pub struct CreateProfileRequest {
    /// Profile name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Field-to-template mappings.
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Arbitrary key-value labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Request to update an existing template profile.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UpdateProfileRequest {
    /// Updated field-to-template mappings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Updated description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Updated labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::HashMap<String, String>>,
}

/// Response from listing templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTemplatesResponse {
    /// List of templates.
    pub templates: Vec<TemplateInfo>,
    /// Total count of results.
    pub count: usize,
}

/// Response from listing template profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListProfilesResponse {
    /// List of template profiles.
    pub profiles: Vec<TemplateProfileInfo>,
    /// Total count of results.
    pub count: usize,
}

/// Request to render a template preview.
#[derive(Debug, Clone, Serialize)]
pub struct RenderPreviewRequest {
    /// Profile name to render.
    pub profile: String,
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// Payload variables to use for rendering.
    pub payload: serde_json::Value,
}

/// Response from rendering a template preview.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderPreviewResponse {
    /// Rendered output keyed by field name.
    pub rendered: std::collections::HashMap<String, String>,
}

// =============================================================================
// Chain Types
// =============================================================================

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
    /// Step status (e.g., "pending", "running", "completed", "failed", "skipped").
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

// =============================================================================
// DAG Types (Chain Visualization)
// =============================================================================

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

// =============================================================================
// DLQ Types (Dead Letter Queue)
// =============================================================================

/// Dead letter queue statistics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqStatsResponse {
    /// Whether the DLQ is enabled.
    pub enabled: bool,
    /// Number of entries currently in the DLQ.
    pub count: usize,
}

/// A single entry in the dead letter queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqEntry {
    /// The original action ID.
    pub action_id: String,
    /// Namespace of the failed action.
    pub namespace: String,
    /// Tenant of the failed action.
    pub tenant: String,
    /// Target provider.
    pub provider: String,
    /// Action type discriminator.
    pub action_type: String,
    /// Error message describing the failure.
    pub error: String,
    /// Number of delivery attempts made.
    pub attempts: u32,
    /// Unix timestamp (seconds) when the entry was added.
    pub timestamp: u64,
}

/// Response from draining the dead letter queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqDrainResponse {
    /// The drained entries.
    pub entries: Vec<DlqEntry>,
    /// Number of entries drained.
    pub count: usize,
}

// =============================================================================
// WASM Plugin Types
// =============================================================================

/// Configuration for a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// Maximum memory in bytes the plugin can use.
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
    /// Maximum execution time in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// List of host functions the plugin is allowed to call.
    #[serde(default)]
    pub allowed_host_functions: Option<Vec<String>>,
}

/// A registered WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPlugin {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin status (e.g., "active", "disabled").
    pub status: String,
    /// Whether the plugin is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Plugin resource configuration.
    #[serde(default)]
    pub config: Option<WasmPluginConfig>,
    /// When the plugin was registered.
    pub created_at: String,
    /// When the plugin was last updated.
    pub updated_at: String,
    /// Number of times the plugin has been invoked.
    #[serde(default)]
    pub invocation_count: u64,
}

/// Request to register a new WASM plugin.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterPluginRequest {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Base64-encoded WASM module bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_bytes: Option<String>,
    /// Path to the WASM file (server-side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_path: Option<String>,
    /// Plugin resource configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<WasmPluginConfig>,
}

/// Response from listing WASM plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPluginsResponse {
    /// List of registered plugins.
    pub plugins: Vec<WasmPlugin>,
    /// Total count.
    pub count: usize,
}

/// Request to test-invoke a WASM plugin.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInvocationRequest {
    /// The function to invoke (default: "evaluate").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// JSON input to pass to the plugin.
    pub input: serde_json::Value,
}

/// Response from test-invoking a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInvocationResponse {
    /// Whether the plugin evaluation returned true (pass) or false (fail).
    pub verdict: bool,
    /// Optional message from the plugin.
    #[serde(default)]
    pub message: Option<String>,
    /// Optional structured metadata from the plugin.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Execution time in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<f64>,
}

// =============================================================================
// Compliance Types (SOC2/HIPAA)
// =============================================================================

/// Current compliance configuration status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceStatus {
    /// The active compliance mode (`"none"`, `"soc2"`, or `"hipaa"`).
    pub mode: String,
    /// Whether audit writes block the dispatch pipeline.
    pub sync_audit_writes: bool,
    /// Whether audit records are immutable (deletes rejected).
    pub immutable_audit: bool,
    /// Whether `SHA-256` hash chaining is enabled for audit records.
    pub hash_chain: bool,
}

/// Result of verifying the integrity of an audit hash chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashChainVerification {
    /// Whether the hash chain is intact (no broken links).
    pub valid: bool,
    /// Total number of records verified.
    pub records_checked: u64,
    /// ID of the first record where the chain broke, if any.
    #[serde(default)]
    pub first_broken_at: Option<String>,
    /// ID of the first record in the verified range.
    #[serde(default)]
    pub first_record_id: Option<String>,
    /// ID of the last record in the verified range.
    #[serde(default)]
    pub last_record_id: Option<String>,
}

/// Request body for hash chain verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyHashChainRequest {
    /// Namespace to verify.
    pub namespace: String,
    /// Tenant to verify.
    pub tenant: String,
    /// Optional start of the time range (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Optional end of the time range (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
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

        let success = BatchResult::Success(ActionOutcome::Executed(ProviderResponse::success(
            serde_json::json!({}),
        )));
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
