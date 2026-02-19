use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use acteon_audit::store::AuditStore;
use acteon_core::{ChainConfig, StateMachineConfig};
use acteon_executor::{DeadLetterQueue, DeadLetterSink, ExecutorConfig};
use acteon_provider::{DynProvider, ProviderRegistry};
use acteon_rules::{Rule, RuleEngine};
use acteon_state::{DistributedLock, StateStore};
use tokio_util::task::TaskTracker;

use acteon_core::EnrichmentConfig;
use acteon_crypto::PayloadEncryptor;
use acteon_llm::LlmEvaluator;
use acteon_provider::ResourceLookup;

use crate::circuit_breaker::{CircuitBreakerConfig, CircuitBreakerRegistry};
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
    embedding: Option<Arc<dyn acteon_rules::EmbeddingEvalSupport>>,
    default_timezone: Option<String>,
    circuit_breaker_default: Option<CircuitBreakerConfig>,
    circuit_breaker_overrides: HashMap<String, CircuitBreakerConfig>,
    stream_buffer_size: usize,
    quota_policies: HashMap<String, acteon_core::QuotaPolicy>,
    retention_policies: HashMap<String, acteon_core::RetentionPolicy>,
    payload_encryptor: Option<Arc<PayloadEncryptor>>,
    wasm_runtime: Option<Arc<dyn acteon_wasm_runtime::WasmPluginRuntime>>,
    compliance_config: Option<acteon_core::ComplianceConfig>,
    enrichments: Vec<EnrichmentConfig>,
    resource_lookups: HashMap<String, Arc<dyn ResourceLookup>>,
    templates: HashMap<(String, String), HashMap<String, acteon_core::Template>>,
    template_profiles: HashMap<(String, String), HashMap<String, acteon_core::TemplateProfile>>,
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
            embedding: None,
            default_timezone: None,
            circuit_breaker_default: None,
            circuit_breaker_overrides: HashMap::new(),
            stream_buffer_size: 1024,
            quota_policies: HashMap::new(),
            retention_policies: HashMap::new(),
            payload_encryptor: None,
            wasm_runtime: None,
            compliance_config: None,
            enrichments: Vec::new(),
            resource_lookups: HashMap::new(),
            templates: HashMap::new(),
            template_profiles: HashMap::new(),
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

    /// Set the embedding support for semantic matching in rule conditions.
    #[must_use]
    pub fn embedding_support(
        mut self,
        support: Arc<dyn acteon_rules::EmbeddingEvalSupport>,
    ) -> Self {
        self.embedding = Some(support);
        self
    }

    /// Set the default IANA timezone for time-based rule conditions.
    ///
    /// When set, `time.*` fields use this timezone unless a rule provides
    /// its own `timezone` override. If not set, UTC is used.
    #[must_use]
    pub fn default_timezone(mut self, tz: impl Into<String>) -> Self {
        self.default_timezone = Some(tz.into());
        self
    }

    /// Enable circuit breakers for all registered providers with the given
    /// default configuration.
    ///
    /// Individual providers can be overridden with
    /// [`circuit_breaker_provider`](Self::circuit_breaker_provider).
    #[must_use]
    pub fn circuit_breaker(mut self, config: CircuitBreakerConfig) -> Self {
        self.circuit_breaker_default = Some(config);
        self
    }

    /// Set a per-provider circuit breaker configuration override.
    ///
    /// This also enables circuit breakers (equivalent to calling
    /// [`circuit_breaker`](Self::circuit_breaker) with default settings
    /// if not already enabled).
    #[must_use]
    pub fn circuit_breaker_provider(
        mut self,
        provider: impl Into<String>,
        config: CircuitBreakerConfig,
    ) -> Self {
        if self.circuit_breaker_default.is_none() {
            self.circuit_breaker_default = Some(CircuitBreakerConfig::default());
        }
        self.circuit_breaker_overrides
            .insert(provider.into(), config);
        self
    }

    /// Set the buffer size for the SSE broadcast channel (default: 1024).
    ///
    /// This controls how many events the broadcast channel can hold before
    /// slow subscribers start missing events (receiving a lagged error).
    #[must_use]
    pub fn stream_buffer_size(mut self, size: usize) -> Self {
        self.stream_buffer_size = size;
        self
    }

    /// Register a quota policy for a tenant.
    ///
    /// The policy is keyed by `"namespace:tenant"`. Multiple policies for the
    /// same tenant replace the previous one.
    #[must_use]
    pub fn quota_policy(mut self, policy: acteon_core::QuotaPolicy) -> Self {
        let key = format!("{}:{}", policy.namespace, policy.tenant);
        self.quota_policies.insert(key, policy);
        self
    }

    /// Register a data retention policy for a tenant.
    ///
    /// The policy is keyed by `"namespace:tenant"`. Multiple policies for the
    /// same tenant replace the previous one.
    #[must_use]
    pub fn retention_policy(mut self, policy: acteon_core::RetentionPolicy) -> Self {
        let key = format!("{}:{}", policy.namespace, policy.tenant);
        self.retention_policies.insert(key, policy);
        self
    }

    /// Set all retention policies at once (replaces any previously added).
    #[must_use]
    pub fn retention_policies(mut self, policies: Vec<acteon_core::RetentionPolicy>) -> Self {
        self.retention_policies = policies
            .into_iter()
            .map(|p| (format!("{}:{}", p.namespace, p.tenant), p))
            .collect();
        self
    }

    /// Set the payload encryptor for encrypting action payloads at rest.
    ///
    /// When set, the gateway encrypts payload-carrying state values before
    /// writing to the state store and decrypts them on read. This protects
    /// scheduled actions, chain state, approval records, and recurring actions.
    #[must_use]
    pub fn payload_encryptor(mut self, enc: Arc<PayloadEncryptor>) -> Self {
        self.payload_encryptor = Some(enc);
        self
    }

    /// Set the WASM plugin runtime for evaluating `WasmCall` expressions in rules.
    ///
    /// When set, rules containing `wasm()` conditions can invoke registered
    /// WASM plugins as part of condition evaluation.
    #[must_use]
    pub fn wasm_runtime(
        mut self,
        runtime: Arc<dyn acteon_wasm_runtime::WasmPluginRuntime>,
    ) -> Self {
        self.wasm_runtime = Some(runtime);
        self
    }

    /// Set the compliance configuration for the gateway.
    ///
    /// When set, enables compliance features such as synchronous audit writes,
    /// immutable audit records, and `SHA-256` hash chaining.
    #[must_use]
    pub fn compliance_config(mut self, config: acteon_core::ComplianceConfig) -> Self {
        self.compliance_config = Some(config);
        self
    }

    /// Register a pre-dispatch enrichment configuration.
    ///
    /// Enrichments are applied in order before rule evaluation. Each enrichment
    /// calls a [`ResourceLookup`] provider to fetch external state and merge it
    /// into the action payload.
    #[must_use]
    pub fn enrichment(mut self, config: EnrichmentConfig) -> Self {
        self.enrichments.push(config);
        self
    }

    /// Register a resource lookup provider for pre-dispatch enrichment.
    ///
    /// The name should match the `lookup_provider` field in enrichment configs.
    #[must_use]
    pub fn resource_lookup(
        mut self,
        name: impl Into<String>,
        lookup: Arc<dyn ResourceLookup>,
    ) -> Self {
        self.resource_lookups.insert(name.into(), lookup);
        self
    }

    /// Register a payload template.
    ///
    /// Templates are stored in a nested map keyed by `(namespace, tenant)` → `name`.
    /// Duplicate names for the same scope replace the previous template.
    #[must_use]
    pub fn template(mut self, template: acteon_core::Template) -> Self {
        let scope = (template.namespace.clone(), template.tenant.clone());
        self.templates
            .entry(scope)
            .or_default()
            .insert(template.name.clone(), template);
        self
    }

    /// Register a template profile.
    ///
    /// Profiles are stored in a nested map keyed by `(namespace, tenant)` → `name`.
    /// Duplicate names for the same scope replace the previous profile.
    #[must_use]
    pub fn template_profile(mut self, profile: acteon_core::TemplateProfile) -> Self {
        let scope = (profile.namespace.clone(), profile.tenant.clone());
        self.template_profiles
            .entry(scope)
            .or_default()
            .insert(profile.name.clone(), profile);
        self
    }

    /// Set all quota policies at once (replaces any previously added).
    #[must_use]
    pub fn quota_policies(mut self, policies: Vec<acteon_core::QuotaPolicy>) -> Self {
        self.quota_policies = policies
            .into_iter()
            .map(|p| (format!("{}:{}", p.namespace, p.tenant), p))
            .collect();
        self
    }

    /// Validate quota policies and wrap them in [`CachedPolicy`] for TTL tracking.
    fn validate_and_wrap_quota_policies(
        policies: HashMap<String, acteon_core::QuotaPolicy>,
    ) -> Result<HashMap<String, crate::gateway::CachedPolicy>, GatewayError> {
        for (key, policy) in &policies {
            if policy.max_actions == 0 {
                return Err(GatewayError::Configuration(format!(
                    "quota policy '{key}' has max_actions = 0"
                )));
            }
            if policy.window.duration_seconds() == 0 {
                return Err(GatewayError::Configuration(format!(
                    "quota policy '{key}' has a zero-duration window"
                )));
            }
        }
        let now = Utc::now();
        Ok(policies
            .into_iter()
            .map(|(k, p)| {
                (
                    k,
                    crate::gateway::CachedPolicy {
                        policy: p,
                        cached_at: now,
                    },
                )
            })
            .collect())
    }

    /// Build and validate the circuit breaker registry if a default config is provided.
    fn build_circuit_breaker_registry(
        default: Option<CircuitBreakerConfig>,
        overrides: &HashMap<String, CircuitBreakerConfig>,
        providers: &ProviderRegistry,
        store: Arc<dyn StateStore>,
        lock: Arc<dyn DistributedLock>,
    ) -> Result<Option<CircuitBreakerRegistry>, GatewayError> {
        let Some(default_config) = default else {
            return Ok(None);
        };
        let mut registry = CircuitBreakerRegistry::new(store, lock);
        let provider_names: Vec<String> = providers
            .list()
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        for name in &provider_names {
            let config = overrides
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(|| default_config.clone());

            config.validate().map_err(|e| {
                GatewayError::Configuration(format!("circuit breaker config for '{name}': {e}"))
            })?;

            if let Some(ref fallback) = config.fallback_provider {
                if fallback == name {
                    return Err(GatewayError::Configuration(format!(
                        "circuit breaker for '{name}' has self-referencing fallback"
                    )));
                }
                if !provider_names.iter().any(|p| p == fallback) {
                    return Err(GatewayError::Configuration(format!(
                        "circuit breaker for '{name}' references unknown fallback provider '{fallback}'"
                    )));
                }
            }

            registry.register(name.as_str(), config);
        }

        // Detect cycles in fallback chains (e.g., A→B→C→A).
        // Build a map of provider → fallback for chain walking.
        let fallback_map: HashMap<&str, &str> = registry
            .providers()
            .into_iter()
            .filter_map(|name| {
                registry
                    .get(name)
                    .and_then(|cb| cb.config().fallback_provider.as_deref())
                    .map(|fb| (name, fb))
            })
            .collect();

        for start in fallback_map.keys() {
            let mut visited = std::collections::HashSet::new();
            visited.insert(*start);
            let mut current = *start;
            while let Some(&next) = fallback_map.get(current) {
                if !visited.insert(next) {
                    return Err(GatewayError::Configuration(format!(
                        "circuit breaker fallback chain contains a cycle: {start} → … → {next} → …"
                    )));
                }
                current = next;
            }
        }

        Ok(Some(registry))
    }

    /// Consume the builder and produce a configured [`Gateway`].
    ///
    /// Returns a [`GatewayError::Configuration`] if required fields
    /// (state store, distributed lock) have not been set.
    #[allow(clippy::too_many_lines)]
    pub fn build(self) -> Result<Gateway, GatewayError> {
        let state = self
            .state
            .ok_or_else(|| GatewayError::Configuration("state store is required".into()))?;

        let lock = self
            .lock
            .ok_or_else(|| GatewayError::Configuration("distributed lock is required".into()))?;

        // Parse the default timezone if provided.
        let default_timezone = self
            .default_timezone
            .as_deref()
            .map(|tz_name| {
                tz_name.parse::<chrono_tz::Tz>().map_err(|_| {
                    GatewayError::Configuration(format!("invalid default_timezone: {tz_name}"))
                })
            })
            .transpose()?;

        let engine = RuleEngine::new(self.rules);

        // Create the DLQ if enabled, wrapping with encryption if configured.
        let dlq: Option<Arc<dyn DeadLetterSink>> = if self.dlq_enabled {
            let raw_dlq = self.dlq.unwrap_or_else(|| Arc::new(DeadLetterQueue::new()));
            if let Some(ref enc) = self.payload_encryptor {
                Some(Arc::new(
                    crate::encrypting_dlq::EncryptingDeadLetterSink::new(raw_dlq, Arc::clone(enc)),
                ))
            } else {
                Some(raw_dlq)
            }
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

        // Build circuit breaker registry if enabled.
        let circuit_breakers = Self::build_circuit_breaker_registry(
            self.circuit_breaker_default,
            &self.circuit_breaker_overrides,
            &self.providers,
            Arc::clone(&state),
            Arc::clone(&lock),
        )?;

        let quota_policies = Self::validate_and_wrap_quota_policies(self.quota_policies)?;

        // Validate the sub-chain reference graph (dangling refs + cycles).
        let chain_graph_errors = acteon_core::validate_chain_graph(&self.chains);
        if !chain_graph_errors.is_empty() {
            return Err(GatewayError::Configuration(format!(
                "invalid chain graph: {}",
                chain_graph_errors.join("; ")
            )));
        }

        // Pre-compute step-name → index maps for each chain config so we
        // don't rebuild them on every step completion during chain advancement.
        let chain_step_indices: HashMap<String, HashMap<String, usize>> = self
            .chains
            .iter()
            .map(|(name, config)| (name.clone(), config.step_index_map()))
            .collect();

        // Create the broadcast channel for SSE event streaming.
        let (stream_tx, _) = tokio::sync::broadcast::channel(self.stream_buffer_size);

        // Wrap the audit store with compliance decorators when configured.
        let mut hash_chain_store: Option<Arc<acteon_audit::HashChainAuditStore>> = None;
        let audit: Option<Arc<dyn AuditStore>> = if let Some(audit_store) = self.audit {
            if let Some(ref compliance) = self.compliance_config {
                let store: Arc<dyn AuditStore> = if compliance.hash_chain {
                    let hcs = Arc::new(acteon_audit::HashChainAuditStore::new(audit_store));
                    hash_chain_store = Some(Arc::clone(&hcs));
                    hcs
                } else {
                    audit_store
                };
                let store: Arc<dyn AuditStore> = if compliance.immutable_audit {
                    Arc::new(acteon_audit::ComplianceAuditStore::new(
                        store,
                        compliance.clone(),
                    ))
                } else {
                    store
                };
                Some(store)
            } else {
                Some(audit_store)
            }
        } else {
            None
        };

        Ok(Gateway {
            state,
            lock,
            engine,
            providers: self.providers,
            executor,
            environment: self.environment,
            metrics: Arc::new(GatewayMetrics::default()),
            audit,
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
            chain_step_indices,
            completed_chain_ttl: self.completed_chain_ttl,
            embedding: self.embedding,
            default_timezone,
            circuit_breakers,
            stream_tx,
            quota_policies: parking_lot::RwLock::new(quota_policies),
            retention_policies: parking_lot::RwLock::new(self.retention_policies),
            payload_encryptor: self.payload_encryptor,
            provider_metrics: Arc::new(crate::metrics::ProviderMetrics::default()),
            wasm_runtime: self.wasm_runtime,
            compliance_config: self.compliance_config,
            hash_chain_store,
            enrichments: self.enrichments,
            resource_lookups: self.resource_lookups,
            templates: parking_lot::RwLock::new(self.templates),
            template_profiles: parking_lot::RwLock::new(self.template_profiles),
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

    /// Minimal mock provider for builder validation tests.
    struct StubProvider(String);
    impl StubProvider {
        fn new(name: &str) -> Self {
            Self(name.into())
        }
    }
    impl acteon_provider::Provider for StubProvider {
        fn name(&self) -> &str {
            &self.0
        }
        async fn execute(
            &self,
            _action: &acteon_core::Action,
        ) -> Result<acteon_core::ProviderResponse, acteon_provider::ProviderError> {
            Ok(acteon_core::ProviderResponse::success(
                serde_json::json!({}),
            ))
        }
        async fn health_check(&self) -> Result<(), acteon_provider::ProviderError> {
            Ok(())
        }
    }

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

    #[test]
    fn build_rejects_invalid_circuit_breaker_config() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let provider = Arc::new(StubProvider::new("email"));
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(provider)
            .circuit_breaker(CircuitBreakerConfig {
                failure_threshold: 0,
                success_threshold: 1,
                recovery_timeout: Duration::from_secs(60),
                fallback_provider: None,
            })
            .build();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failure_threshold")
        );
    }

    #[test]
    fn build_rejects_unknown_fallback_provider() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let provider = Arc::new(StubProvider::new("email"));
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(provider)
            .circuit_breaker_provider(
                "email",
                CircuitBreakerConfig {
                    fallback_provider: Some("nonexistent".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .build();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unknown fallback provider")
        );
    }

    #[test]
    fn build_rejects_self_referencing_fallback() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let provider = Arc::new(StubProvider::new("email"));
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(provider)
            .circuit_breaker_provider(
                "email",
                CircuitBreakerConfig {
                    fallback_provider: Some("email".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .build();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("self-referencing fallback")
        );
    }

    #[test]
    fn build_rejects_fallback_cycle() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(Arc::new(StubProvider::new("a")))
            .provider(Arc::new(StubProvider::new("b")))
            .provider(Arc::new(StubProvider::new("c")))
            .circuit_breaker_provider(
                "a",
                CircuitBreakerConfig {
                    fallback_provider: Some("b".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .circuit_breaker_provider(
                "b",
                CircuitBreakerConfig {
                    fallback_provider: Some("c".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .circuit_breaker_provider(
                "c",
                CircuitBreakerConfig {
                    fallback_provider: Some("a".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .build();
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("cycle"),
            "should detect A→B→C→A cycle"
        );
    }

    #[test]
    fn build_rejects_quota_policy_zero_max_actions() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let policy = acteon_core::QuotaPolicy {
            id: "q-bad".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            max_actions: 0,
            window: acteon_core::quota::QuotaWindow::Daily,
            overage_behavior: acteon_core::quota::OverageBehavior::Block,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .quota_policy(policy)
            .build();
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("max_actions = 0"),
            "should reject quota with zero max_actions"
        );
    }

    #[test]
    fn build_rejects_quota_policy_zero_window() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let policy = acteon_core::QuotaPolicy {
            id: "q-bad2".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            max_actions: 100,
            window: acteon_core::quota::QuotaWindow::Custom { seconds: 0 },
            overage_behavior: acteon_core::quota::OverageBehavior::Block,
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .quota_policy(policy)
            .build();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("zero-duration window"),
            "should reject quota with zero-duration window"
        );
    }

    #[test]
    fn build_accepts_valid_fallback_chain() {
        let store = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let result = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .provider(Arc::new(StubProvider::new("a")))
            .provider(Arc::new(StubProvider::new("b")))
            .provider(Arc::new(StubProvider::new("c")))
            .circuit_breaker_provider(
                "a",
                CircuitBreakerConfig {
                    fallback_provider: Some("b".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .circuit_breaker_provider(
                "b",
                CircuitBreakerConfig {
                    fallback_provider: Some("c".into()),
                    ..CircuitBreakerConfig::default()
                },
            )
            .build();
        assert!(result.is_ok(), "A→B→C (no cycle) should be accepted");
    }

    #[test]
    fn builder_wasm_runtime() {
        use acteon_wasm_runtime::MockWasmRuntime;

        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let wasm = Arc::new(MockWasmRuntime::new(true));

        let gateway = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .wasm_runtime(wasm)
            .build()
            .unwrap();

        assert!(gateway.wasm_runtime().is_some());
    }

    #[test]
    fn builder_without_wasm_runtime() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let gateway = GatewayBuilder::new()
            .state(store)
            .lock(lock)
            .build()
            .unwrap();

        assert!(gateway.wasm_runtime().is_none());
    }
}
