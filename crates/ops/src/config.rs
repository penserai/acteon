//! Configuration for the operations layer.

use std::time::Duration;

/// Configuration for connecting to an Acteon server.
#[derive(Debug, Clone)]
pub struct OpsConfig {
    /// Acteon server endpoint URL (e.g. `http://localhost:8080`).
    pub endpoint: String,
    /// API key for authentication.
    pub api_key: Option<String>,
    /// Request timeout.
    pub timeout: Option<Duration>,
}

impl OpsConfig {
    /// Create a new configuration with defaults.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: None,
            timeout: None,
        }
    }

    /// Create configuration from environment variables.
    ///
    /// Reads:
    /// - `ACTEON_ENDPOINT` (required, defaults to `http://localhost:8080`)
    /// - `ACTEON_API_KEY` (optional)
    /// - `ACTEON_TIMEOUT_SECS` (optional, default 30)
    pub fn from_env() -> Self {
        let endpoint = std::env::var("ACTEON_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());
        let api_key = std::env::var("ACTEON_API_KEY").ok();
        let timeout = std::env::var("ACTEON_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs);

        Self {
            endpoint,
            api_key,
            timeout,
        }
    }

    /// Override the API key.
    #[must_use]
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Override the timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl Default for OpsConfig {
    fn default() -> Self {
        Self::from_env()
    }
}
