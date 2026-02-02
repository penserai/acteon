use std::collections::HashMap;
use std::sync::Arc;

use acteon_audit::store::AuditStore;
use acteon_executor::ExecutorConfig;
use acteon_provider::{DynProvider, ProviderRegistry};
use acteon_rules::{Rule, RuleEngine};
use acteon_state::{DistributedLock, StateStore};

use crate::error::GatewayError;
use crate::gateway::Gateway;
use crate::metrics::GatewayMetrics;

/// Fluent builder for constructing a [`Gateway`] instance.
///
/// At minimum, a [`StateStore`] and [`DistributedLock`] implementation must
/// be supplied. All other fields have sensible defaults (empty rules, empty
/// providers, default executor config).
pub struct GatewayBuilder {
    state: Option<Arc<dyn StateStore>>,
    lock: Option<Arc<dyn DistributedLock>>,
    rules: Vec<Rule>,
    providers: ProviderRegistry,
    executor_config: ExecutorConfig,
    environment: HashMap<String, String>,
    audit: Option<Arc<dyn AuditStore>>,
    audit_ttl_seconds: Option<u64>,
    audit_store_payload: bool,
}

impl GatewayBuilder {
    /// Create a new builder with all optional fields set to their defaults.
    pub fn new() -> Self {
        Self {
            state: None,
            lock: None,
            rules: Vec::new(),
            providers: ProviderRegistry::new(),
            executor_config: ExecutorConfig::default(),
            environment: HashMap::new(),
            audit: None,
            audit_ttl_seconds: None,
            audit_store_payload: true,
        }
    }

    /// Set the state store implementation.
    #[must_use]
    pub fn state(mut self, store: Arc<dyn StateStore>) -> Self {
        self.state = Some(store);
        self
    }

    /// Set the distributed lock implementation.
    #[must_use]
    pub fn lock(mut self, lock: Arc<dyn DistributedLock>) -> Self {
        self.lock = Some(lock);
        self
    }

    /// Set the rules to be evaluated by the gateway's rule engine.
    #[must_use]
    pub fn rules(mut self, rules: Vec<Rule>) -> Self {
        self.rules = rules;
        self
    }

    /// Register a provider with the gateway.
    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn DynProvider>) -> Self {
        self.providers.register(provider);
        self
    }

    /// Set the executor configuration (retries, concurrency, timeouts).
    #[must_use]
    pub fn executor_config(mut self, config: ExecutorConfig) -> Self {
        self.executor_config = config;
        self
    }

    /// Add a single environment variable for rule evaluation.
    #[must_use]
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Set the audit store for recording dispatch events.
    #[must_use]
    pub fn audit(mut self, store: Arc<dyn AuditStore>) -> Self {
        self.audit = Some(store);
        self
    }

    /// Set the TTL (in seconds) for audit records.
    #[must_use]
    pub fn audit_ttl_seconds(mut self, seconds: u64) -> Self {
        self.audit_ttl_seconds = Some(seconds);
        self
    }

    /// Set whether to store the action payload in audit records.
    #[must_use]
    pub fn audit_store_payload(mut self, store: bool) -> Self {
        self.audit_store_payload = store;
        self
    }

    /// Consume the builder and produce a configured [`Gateway`].
    ///
    /// Returns a [`GatewayError::Configuration`] if required fields
    /// (state store, distributed lock) have not been set.
    pub fn build(self) -> Result<Gateway, GatewayError> {
        let state = self
            .state
            .ok_or_else(|| GatewayError::Configuration("state store is required".into()))?;

        let lock = self
            .lock
            .ok_or_else(|| GatewayError::Configuration("distributed lock is required".into()))?;

        let engine = RuleEngine::new(self.rules);
        let executor = acteon_executor::ActionExecutor::new(self.executor_config);

        Ok(Gateway {
            state,
            lock,
            engine,
            providers: self.providers,
            executor,
            environment: self.environment,
            metrics: Arc::new(GatewayMetrics::default()),
            audit: self.audit,
            audit_ttl_seconds: self.audit_ttl_seconds,
            audit_store_payload: self.audit_store_payload,
        })
    }
}

impl Default for GatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

    #[test]
    fn build_missing_state_returns_error() {
        let lock = Arc::new(MemoryDistributedLock::new());
        let result = GatewayBuilder::new().lock(lock).build();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("state store is required"));
    }

    #[test]
    fn build_missing_lock_returns_error() {
        let store = Arc::new(MemoryStateStore::new());
        let result = GatewayBuilder::new().state(store).build();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("distributed lock is required"));
    }

    #[test]
    fn build_with_required_fields_succeeds() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let result = GatewayBuilder::new().state(store).lock(lock).build();
        assert!(result.is_ok());
    }
}
