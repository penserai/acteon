//! Common operations layer for the Acteon CLI and MCP server.
//!
//! Wraps [`acteon_client::ActeonClient`] with configuration management.
//! Both the CLI and MCP server build on top of this crate.

mod config;
mod error;

pub use config::OpsConfig;
pub use error::OpsError;

use acteon_client::{ActeonClient, ActeonClientBuilder};
use std::sync::Arc;

/// Re-export client and core types for consumers.
pub use acteon_client;
pub use acteon_core;

/// High-level operations client for Acteon.
///
/// Wraps the HTTP client with configuration management and provides
/// the shared interface consumed by both the CLI and MCP server.
#[derive(Clone)]
pub struct OpsClient {
    inner: Arc<ActeonClient>,
}

impl OpsClient {
    /// Create a new operations client from configuration.
    pub fn from_config(config: &OpsConfig) -> Result<Self, OpsError> {
        let mut builder = ActeonClientBuilder::new(&config.endpoint);

        if let Some(ref timeout) = config.timeout {
            builder = builder.timeout(*timeout);
        }

        if let Some(ref api_key) = config.api_key {
            builder = builder.api_key(api_key);
        }

        let client = builder
            .build()
            .map_err(|e| OpsError::Configuration(e.to_string()))?;

        Ok(Self {
            inner: Arc::new(client),
        })
    }

    /// Access the underlying HTTP client directly.
    ///
    /// Use this when you need to call a specific client method that
    /// is not wrapped by the ops layer.
    pub fn client(&self) -> &ActeonClient {
        &self.inner
    }
}
