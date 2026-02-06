use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acteon_audit::store::AuditStore;
use acteon_core::{ChainConfig, StateMachineConfig};
use acteon_executor::{DeadLetterQueue, DeadLetterSink, ExecutorConfig};
use acteon_provider::{DynProvider, ProviderRegistry};
use acteon_rules::{Rule, RuleEngine};
use acteon_state::{DistributedLock, StateStore};
use tokio_util::task::TaskTracker;

use acteon_llm::LlmEvaluator;

use crate::error::GatewayError;
use crate::gateway::{ApprovalKeySet, Gateway};
use crate::group_manager::GroupManager;
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
    dlq: Option<Arc<dyn DeadLetterSink>>,
    dlq_enabled: bool,
    state_machines: HashMap<String, StateMachineConfig>,
    group_manager: Option<Arc<GroupManager>>,
    external_url: Option<String>,
    approval_secret: Option<Vec<u8>>,
    approval_keys: Option<ApprovalKeySet>,
    llm_evaluator: Option<Arc<dyn LlmEvaluator>>,
    llm_policy: String,
    llm_policies: HashMap<String, String>,
    llm_fail_open: bool,
    chains: HashMap<String, ChainConfig>,
    completed_chain_ttl: Option<Duration>,
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
            dlq: None,
            dlq_enabled: false,
            state_machines: HashMap::new(),
            group_manager: None,
            external_url: None,
            approval_secret: None,
            approval_keys: None,
            llm_evaluator: None,
            llm_policy: String::new(),
            llm_policies: HashMap::new(),
            llm_fail_open: true,
            chains: HashMap::new(),
            completed_chain_ttl: None,
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

    /// Enable the dead-letter queue for failed actions.
    ///
    /// When enabled, actions that exhaust all retry attempts are stored in the
    /// DLQ for later inspection or reprocessing. By default, an in-memory DLQ
    /// is used. Use [`dlq_sink`](Self::dlq_sink) to provide a custom implementation.
    #[must_use]
    pub fn dlq_enabled(mut self, enabled: bool) -> Self {
        self.dlq_enabled = enabled;
        self
    }

    /// Set a custom dead-letter queue sink.
    ///
    /// This also enables the DLQ. Use this to provide a persistent DLQ
    /// implementation (e.g., Redis, `PostgreSQL`).
    #[must_use]
    pub fn dlq_sink(mut self, sink: Arc<dyn DeadLetterSink>) -> Self {
        self.dlq = Some(sink);
        self.dlq_enabled = true;
        self
    }

    /// Register a state machine configuration.
    #[must_use]
    pub fn state_machine(mut self, config: StateMachineConfig) -> Self {
        self.state_machines.insert(config.name.clone(), config);
        self
    }

    /// Set a shared group manager.
    ///
    /// Use this when you need to share the group manager with a
    /// [`BackgroundProcessor`](crate::background::BackgroundProcessor) for
    /// automatic group flushing.
    #[must_use]
    pub fn group_manager(mut self, manager: Arc<GroupManager>) -> Self {
        self.group_manager = Some(manager);
        self
    }

    /// Set the external URL for building approval links.
    ///
    /// This URL is used to construct approve/reject URLs in approval
    /// notifications. If not set, defaults to `http://localhost:8080`.
    #[must_use]
    pub fn external_url(mut self, url: impl Into<String>) -> Self {
        self.external_url = Some(url.into());
        self
    }

    /// Set the HMAC secret used to sign approval URLs.
    ///
    /// If not set, a random 32-byte secret is generated automatically.
    /// Pass a stable secret for approval URLs that survive server restarts.
    ///
    /// For key rotation support, use [`approval_keys`](Self::approval_keys) instead.
    #[must_use]
    pub fn approval_secret(mut self, secret: impl Into<Vec<u8>>) -> Self {
        self.approval_secret = Some(secret.into());
        self
    }

    /// Set the HMAC key set used to sign and verify approval URLs.
    ///
    /// The first key in the set is the current signing key. All keys are
    /// tried during verification, enabling zero-downtime key rotation.
    /// This takes precedence over [`approval_secret`](Self::approval_secret).
    #[must_use]
    pub fn approval_keys(mut self, keys: ApprovalKeySet) -> Self {
        self.approval_keys = Some(keys);
        self
    }

    /// Set the LLM evaluator for guardrail checks.
    ///
    /// When set, actions that pass rule evaluation are additionally checked
    /// by the LLM before execution. Actions already denied or suppressed by
    /// rules skip the LLM call.
    #[must_use]
    pub fn llm_evaluator(mut self, evaluator: Arc<dyn LlmEvaluator>) -> Self {
        self.llm_evaluator = Some(evaluator);
        self
    }

    /// Set the policy prompt sent to the LLM guardrail.
    #[must_use]
    pub fn llm_policy(mut self, policy: impl Into<String>) -> Self {
        self.llm_policy = policy.into();
        self
    }

    /// Set per-action-type LLM policy overrides.
    ///
    /// Keys are action type strings, values are policy prompts.
    /// These take precedence over the global policy but are overridden
    /// by per-rule metadata `llm_policy` entries.
    #[must_use]
    pub fn llm_policies(mut self, policies: HashMap<String, String>) -> Self {
        self.llm_policies = policies;
        self
    }

    /// Set whether the LLM guardrail fails open (default: `true`).
    ///
    /// When `true`, LLM evaluation errors allow the action to proceed.
    /// When `false`, errors cause the action to be denied.
    #[must_use]
    pub fn llm_fail_open(mut self, fail_open: bool) -> Self {
        self.llm_fail_open = fail_open;
        self
    }

    /// Register a task chain configuration.
    #[must_use]
    pub fn chain(mut self, config: ChainConfig) -> Self {
        self.chains.insert(config.name.clone(), config);
        self
    }

    /// Set the TTL for completed/failed/cancelled chain state records.
    ///
    /// After a chain reaches a terminal status, its state record is kept for
    /// this duration for audit purposes. If not set, terminal chain state
    /// persists indefinitely.
    #[must_use]
    pub fn completed_chain_ttl(mut self, ttl: Duration) -> Self {
        self.completed_chain_ttl = Some(ttl);
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

        // Create the DLQ if enabled.
        let dlq: Option<Arc<dyn DeadLetterSink>> = if self.dlq_enabled {
            self.dlq.or_else(|| Some(Arc::new(DeadLetterQueue::new())))
        } else {
            None
        };

        // Create the executor with optional DLQ.
        let executor = if let Some(ref dlq_sink) = dlq {
            acteon_executor::ActionExecutor::with_dlq(self.executor_config, Arc::clone(dlq_sink))
        } else {
            acteon_executor::ActionExecutor::new(self.executor_config)
        };

        // Use provided group manager or create a new one.
        let group_manager = self
            .group_manager
            .unwrap_or_else(|| Arc::new(GroupManager::new()));

        // Use provided key set, or wrap a single secret, or generate a random key.
        let approval_keys = if let Some(keys) = self.approval_keys {
            keys
        } else if let Some(secret) = self.approval_secret {
            ApprovalKeySet::from_single(secret)
        } else {
            let a = uuid::Uuid::new_v4();
            let b = uuid::Uuid::new_v4();
            let mut secret = Vec::with_capacity(32);
            secret.extend_from_slice(a.as_bytes());
            secret.extend_from_slice(b.as_bytes());
            ApprovalKeySet::from_single(secret)
        };

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
            audit_tracker: TaskTracker::new(),
            dlq,
            state_machines: self.state_machines,
            group_manager,
            external_url: self.external_url,
            approval_keys,
            llm_evaluator: self.llm_evaluator,
            llm_policy: self.llm_policy,
            llm_policies: self.llm_policies,
            llm_fail_open: self.llm_fail_open,
            chains: self.chains,
            completed_chain_ttl: self.completed_chain_ttl,
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
