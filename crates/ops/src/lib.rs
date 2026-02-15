//! Common operations layer for the Acteon CLI and MCP server.
//!
//! Wraps [`acteon_client::ActeonClient`] with configuration management.
//! Both the CLI and MCP server build on top of this crate.

mod config;
mod error;
pub mod test_rules;

pub use config::OpsConfig;
pub use error::OpsError;

use acteon_client::{
    ActeonClient, ActeonClientBuilder, AuditPage, AuditQuery, EvaluateRulesOptions,
    EventListResponse, EventQuery, ListChainsResponse, RuleEvaluationTrace, RuleInfo,
    TransitionResponse,
};
use acteon_core::{
    Action, ActionOutcome, CircuitBreakerActionResponse, ListCircuitBreakersResponse,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Re-export client and core types for consumers.
pub mod acteon_client {
    pub use acteon_client::*;
}
pub mod acteon_core {
    pub use acteon_core::*;
}

/// Options for dispatching an action.
#[derive(Debug, Clone, Default)]
pub struct DispatchOptions {
    pub metadata: HashMap<String, String>,
    pub dry_run: bool,
}

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
    pub fn client(&self) -> &ActeonClient {
        &self.inner
    }

    // =========================================================================
    // High-level Operations
    // =========================================================================

    /// Dispatch an action with optional metadata and dry-run mode.
    pub async fn dispatch(
        &self,
        namespace: String,
        tenant: String,
        provider: String,
        action_type: String,
        payload: serde_json::Value,
        options: DispatchOptions,
    ) -> Result<ActionOutcome, OpsError> {
        let mut action = Action::new(namespace, tenant, provider, &action_type, payload);
        if !options.metadata.is_empty() {
            action.metadata.labels = options.metadata;
        }

        let outcome = if options.dry_run {
            self.inner.dispatch_dry_run(&action).await?
        } else {
            self.inner.dispatch(&action).await?
        };

        Ok(outcome)
    }

    /// Query the audit trail.
    pub async fn query_audit(&self, query: AuditQuery) -> Result<AuditPage, OpsError> {
        Ok(self.inner.query_audit(&query).await?)
    }

    /// List routing rules.
    pub async fn list_rules(&self) -> Result<Vec<RuleInfo>, OpsError> {
        Ok(self.inner.list_rules().await?)
    }

    /// Evaluate rules against a test action.
    pub async fn evaluate_rules(
        &self,
        namespace: String,
        tenant: String,
        provider: String,
        action_type: String,
        payload: serde_json::Value,
        include_disabled: bool,
    ) -> Result<RuleEvaluationTrace, OpsError> {
        let action = Action::new(namespace, tenant, provider, &action_type, payload);
        let options = EvaluateRulesOptions {
            include_disabled,
            ..EvaluateRulesOptions::default()
        };

        Ok(self.inner.evaluate_rules(&action, &options).await?)
    }

    /// Transition a stateful event.
    pub async fn transition_event(
        &self,
        fingerprint: String,
        to_state: String,
        namespace: String,
        tenant: String,
    ) -> Result<TransitionResponse, OpsError> {
        Ok(self
            .inner
            .transition_event(&fingerprint, &to_state, &namespace, &tenant)
            .await?)
    }

    /// List stateful events.
    pub async fn list_events(&self, query: EventQuery) -> Result<EventListResponse, OpsError> {
        Ok(self.inner.list_events(&query).await?)
    }

    /// List action chains.
    pub async fn list_chains(
        &self,
        namespace: String,
        tenant: String,
        status: Option<String>,
    ) -> Result<ListChainsResponse, OpsError> {
        Ok(self
            .inner
            .list_chains(&namespace, &tenant, status.as_deref())
            .await?)
    }

    /// Enable or disable a rule.
    pub async fn set_rule_enabled(&self, rule_name: String, enabled: bool) -> Result<(), OpsError> {
        Ok(self.inner.set_rule_enabled(&rule_name, enabled).await?)
    }

    /// Check health.
    pub async fn health(&self) -> Result<bool, OpsError> {
        Ok(self.inner.health().await?)
    }

    /// List all circuit breakers.
    pub async fn list_circuit_breakers(&self) -> Result<ListCircuitBreakersResponse, OpsError> {
        Ok(self.inner.list_circuit_breakers().await?)
    }

    /// Trip a circuit breaker.
    pub async fn trip_circuit_breaker(
        &self,
        provider: String,
    ) -> Result<CircuitBreakerActionResponse, OpsError> {
        Ok(self.inner.trip_circuit_breaker(&provider).await?)
    }

    /// List retention policies, optionally filtered by namespace and tenant.
    pub async fn list_retention(
        &self,
        namespace: Option<String>,
        tenant: Option<String>,
    ) -> Result<acteon_client::ListRetentionResponse, OpsError> {
        Ok(self
            .inner
            .list_retention(namespace.as_deref(), tenant.as_deref())
            .await?)
    }

    /// Reset a circuit breaker.
    pub async fn reset_circuit_breaker(
        &self,
        provider: String,
    ) -> Result<CircuitBreakerActionResponse, OpsError> {
        Ok(self.inner.reset_circuit_breaker(&provider).await?)
    }
}
