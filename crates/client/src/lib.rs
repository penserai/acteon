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

// Domain-specific modules containing `impl ActeonClient` blocks and model types.
mod approvals;
mod audit;
mod chains;
mod circuit_breakers;
mod compliance;
mod dispatch;
mod dlq;
mod events;
mod groups;
mod plugins;
mod providers;
mod quotas;
mod recurring;
mod retention;
mod rules;
mod streaming;
mod templates;

pub use error::Error;
pub use stream::{EventStream, StreamFilter, StreamItem};

// Re-export core attachment type so callers don't need a direct `acteon_core` dependency.
pub use acteon_core::Attachment;

// Re-export all public types from domain modules so the public API is unchanged.
pub use approvals::*;
pub use audit::*;
pub use chains::*;
pub use compliance::*;
pub use dispatch::*;
pub use dlq::*;
pub use events::*;
pub use groups::*;
pub use plugins::*;
pub use quotas::*;
pub use recurring::*;
pub use retention::*;
pub use rules::*;
pub use templates::*;

use std::time::Duration;

use reqwest::Client;

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for the Acteon action gateway.
///
/// Provides methods for dispatching actions, managing rules, and querying audit logs
/// via the REST API.
#[derive(Debug, Clone)]
pub struct ActeonClient {
    pub(crate) client: Client,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
}

/// Builder for configuring an [`ActeonClient`].
#[derive(Debug)]
pub struct ActeonClientBuilder {
    base_url: String,
    timeout: Duration,
    api_key: Option<String>,
    client: Option<Client>,
    ca_cert_path: Option<String>,
    client_cert_path: Option<String>,
    client_key_path: Option<String>,
    danger_accept_invalid_certs: bool,
}

impl ActeonClientBuilder {
    /// Create a new builder with the given base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            timeout: DEFAULT_TIMEOUT,
            api_key: None,
            client: None,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            danger_accept_invalid_certs: false,
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

    /// Set a custom CA certificate file (PEM) for server verification.
    ///
    /// When set, only certificates signed by this CA will be trusted.
    /// If not set, the system's default root certificates are used.
    #[must_use]
    pub fn ca_cert_path(mut self, path: impl Into<String>) -> Self {
        self.ca_cert_path = Some(path.into());
        self
    }

    /// Set client certificate and key files (PEM) for mTLS.
    ///
    /// Both paths must be provided for client certificate authentication.
    #[must_use]
    pub fn client_cert(
        mut self,
        cert_path: impl Into<String>,
        key_path: impl Into<String>,
    ) -> Self {
        self.client_cert_path = Some(cert_path.into());
        self.client_key_path = Some(key_path.into());
        self
    }

    /// Skip certificate verification (dev/test only).
    ///
    /// # Warning
    ///
    /// This completely disables TLS certificate validation. Only use in
    /// development or testing environments.
    #[must_use]
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.danger_accept_invalid_certs = accept;
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
        let client = if let Some(c) = self.client {
            c
        } else {
            let mut builder = Client::builder()
                .timeout(self.timeout)
                .danger_accept_invalid_certs(self.danger_accept_invalid_certs);

            if let Some(ref ca_path) = self.ca_cert_path {
                let ca_pem = std::fs::read(ca_path).map_err(|e| {
                    Error::Configuration(format!("failed to read CA cert {ca_path}: {e}"))
                })?;
                let ca_cert = reqwest::Certificate::from_pem(&ca_pem)
                    .map_err(|e| Error::Configuration(format!("invalid CA cert: {e}")))?;
                builder = builder.add_root_certificate(ca_cert);
            }

            if let (Some(cert_path), Some(key_path)) =
                (&self.client_cert_path, &self.client_key_path)
            {
                let cert_pem = std::fs::read(cert_path).map_err(|e| {
                    Error::Configuration(format!("failed to read client cert {cert_path}: {e}"))
                })?;
                let key_pem = std::fs::read(key_path).map_err(|e| {
                    Error::Configuration(format!("failed to read client key {key_path}: {e}"))
                })?;
                let mut combined = cert_pem;
                combined.push(b'\n');
                combined.extend_from_slice(&key_pem);
                let identity = reqwest::Identity::from_pem(&combined)
                    .map_err(|e| Error::Configuration(format!("invalid client identity: {e}")))?;
                builder = builder.identity(identity);
            }

            builder
                .build()
                .map_err(|e| Error::Configuration(e.to_string()))?
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
    pub(crate) fn add_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
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
        use acteon_core::{ActionOutcome, ProviderResponse};

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
