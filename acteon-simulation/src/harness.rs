//! Simulation harness for orchestrating multi-node tests.

use std::collections::HashMap;
use std::sync::Arc;

use acteon_audit_memory::MemoryAuditStore;
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayError;
use acteon_provider::DynProvider;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_state::StateStore;
use acteon_state::DistributedLock;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

use crate::cluster::{
    AuditBackendConfig, PortAllocator, ServerNode, SimulationConfig, StateBackendConfig,
};
use crate::error::SimulationError;
use crate::provider::RecordingProvider;

/// Main orchestrator for simulation tests.
///
/// The `SimulationHarness` manages a cluster of Acteon nodes and provides
/// utilities for dispatching actions, accessing recording providers, and
/// verifying test outcomes.
pub struct SimulationHarness {
    nodes: Vec<ServerNode>,
    providers: HashMap<String, Arc<RecordingProvider>>,
    port_allocator: PortAllocator,
    #[allow(dead_code)]
    shared_state: Option<Arc<dyn StateStore>>,
}

impl SimulationHarness {
    /// Start a simulation cluster with the given configuration.
    pub async fn start(config: SimulationConfig) -> Result<Self, SimulationError> {
        let port_allocator = PortAllocator::new();

        // Parse rules from YAML
        let rules = Self::parse_rules(&config.rules)?;

        // Create recording providers
        let mut providers: HashMap<String, Arc<RecordingProvider>> = HashMap::new();
        for name in &config.providers {
            providers.insert(name.clone(), Arc::new(RecordingProvider::new(name)));
        }

        // Convert to DynProvider references
        let provider_refs: Vec<Arc<dyn DynProvider>> = providers
            .values()
            .map(|p| Arc::clone(p) as Arc<dyn DynProvider>)
            .collect();

        // Create shared state if needed
        let shared_state: Option<Arc<dyn StateStore>> = if config.shared_state || config.nodes > 1 {
            match &config.state_backend {
                StateBackendConfig::Memory => Some(Arc::new(MemoryStateStore::new())),
                #[cfg(feature = "redis")]
                StateBackendConfig::Redis { url: _, prefix: _ } => {
                    // For now, fall back to memory even when Redis is configured
                    // A real implementation would create a Redis connection here
                    Some(Arc::new(MemoryStateStore::new()))
                }
            }
        } else {
            None
        };

        // Create shared lock
        let shared_lock: Arc<dyn DistributedLock> = Arc::new(MemoryDistributedLock::new());

        // Create nodes
        let mut nodes = Vec::with_capacity(config.nodes);
        for i in 0..config.nodes {
            let addr = port_allocator
                .allocate()
                .ok_or(SimulationError::PortExhausted)?;

            let state: Arc<dyn StateStore> = shared_state
                .as_ref()
                .map(Arc::clone)
                .unwrap_or_else(|| Arc::new(MemoryStateStore::new()));

            let audit: Option<Arc<dyn acteon_audit::AuditStore>> = match &config.audit_backend {
                AuditBackendConfig::Memory => Some(Arc::new(MemoryAuditStore::new())),
                AuditBackendConfig::Disabled => None,
            };

            let node = ServerNode::new(
                format!("node-{i}"),
                addr,
                state,
                Arc::clone(&shared_lock),
                rules.clone(),
                provider_refs.clone(),
                audit,
                config.environment.clone(),
            )?;

            nodes.push(node);
        }

        Ok(Self {
            nodes,
            providers,
            port_allocator,
            shared_state,
        })
    }

    /// Create a single-node harness with in-memory backends.
    pub async fn single_node_memory() -> Result<Self, SimulationError> {
        Self::start(
            SimulationConfig::builder()
                .nodes(1)
                .state_backend(StateBackendConfig::Memory)
                .audit_backend(AuditBackendConfig::Memory)
                .build(),
        )
        .await
    }

    /// Create a multi-node harness with shared memory state.
    pub async fn multi_node_memory(count: usize) -> Result<Self, SimulationError> {
        Self::start(
            SimulationConfig::builder()
                .nodes(count)
                .shared_state(true)
                .state_backend(StateBackendConfig::Memory)
                .audit_backend(AuditBackendConfig::Memory)
                .build(),
        )
        .await
    }

    /// Create a multi-node harness with Redis-backed state.
    #[cfg(feature = "redis")]
    pub async fn multi_node_redis(count: usize, redis_url: &str) -> Result<Self, SimulationError> {
        Self::start(
            SimulationConfig::builder()
                .nodes(count)
                .shared_state(true)
                .state_backend(StateBackendConfig::Redis {
                    url: redis_url.to_string(),
                    prefix: Some("sim".to_string()),
                })
                .audit_backend(AuditBackendConfig::Memory)
                .build(),
        )
        .await
    }

    /// Get a reference to a recording provider by name.
    pub fn provider(&self, name: &str) -> Option<&Arc<RecordingProvider>> {
        self.providers.get(name)
    }

    /// Get all recording providers.
    pub fn providers(&self) -> &HashMap<String, Arc<RecordingProvider>> {
        &self.providers
    }

    /// Get a reference to a node by index.
    pub fn node(&self, index: usize) -> Option<&ServerNode> {
        self.nodes.get(index)
    }

    /// Get the number of nodes in the cluster.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Dispatch an action to the first node.
    pub async fn dispatch(&self, action: &Action) -> Result<ActionOutcome, GatewayError> {
        self.dispatch_to(0, action).await
    }

    /// Dispatch an action to a specific node.
    pub async fn dispatch_to(
        &self,
        node_index: usize,
        action: &Action,
    ) -> Result<ActionOutcome, GatewayError> {
        let node = self.nodes.get(node_index).ok_or_else(|| {
            GatewayError::Configuration(format!("node {node_index} not found"))
        })?;

        node.dispatch(action.clone()).await
    }

    /// Dispatch a batch of actions to the first node.
    pub async fn dispatch_batch(
        &self,
        actions: &[Action],
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.dispatch_batch_to(0, actions).await
    }

    /// Dispatch a batch of actions to a specific node.
    pub async fn dispatch_batch_to(
        &self,
        node_index: usize,
        actions: &[Action],
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        let Some(node) = self.nodes.get(node_index) else {
            return actions
                .iter()
                .map(|_| {
                    Err(GatewayError::Configuration(format!(
                        "node {node_index} not found"
                    )))
                })
                .collect();
        };

        node.dispatch_batch(actions.to_vec()).await
    }

    /// Reset all recording providers, clearing captured calls.
    pub fn reset_recordings(&self) {
        for provider in self.providers.values() {
            provider.clear();
        }
    }

    /// Teardown the simulation, stopping all nodes.
    pub async fn teardown(mut self) -> Result<(), SimulationError> {
        for node in &mut self.nodes {
            node.stop().await?;
            self.port_allocator.release(node.addr.port());
        }
        Ok(())
    }

    /// Parse YAML rule strings into Rule objects.
    fn parse_rules(yaml_strings: &[String]) -> Result<Vec<Rule>, SimulationError> {
        let frontend = YamlFrontend;
        let mut rules = Vec::new();

        for yaml in yaml_strings {
            let parsed = acteon_rules::RuleFrontend::parse(&frontend, yaml)
                .map_err(|e| SimulationError::Configuration(format!("rule parse error: {e}")))?;
            rules.extend(parsed);
        }

        Ok(rules)
    }
}

/// Builder for `SimulationHarness` with fluent API.
#[derive(Default)]
pub struct SimulationHarnessBuilder {
    config: SimulationConfig,
}

impl SimulationHarnessBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of nodes.
    #[must_use]
    pub fn nodes(mut self, count: usize) -> Self {
        self.config.nodes = count;
        self
    }

    /// Enable or disable shared state.
    #[must_use]
    pub fn shared_state(mut self, shared: bool) -> Self {
        self.config.shared_state = shared;
        self
    }

    /// Set the state backend.
    #[must_use]
    pub fn state_backend(mut self, backend: StateBackendConfig) -> Self {
        self.config.state_backend = backend;
        self
    }

    /// Set the audit backend.
    #[must_use]
    pub fn audit_backend(mut self, backend: AuditBackendConfig) -> Self {
        self.config.audit_backend = backend;
        self
    }

    /// Add a YAML rule definition.
    #[must_use]
    pub fn add_rule_yaml(mut self, yaml: impl Into<String>) -> Self {
        self.config.rules.push(yaml.into());
        self
    }

    /// Add a recording provider by name.
    #[must_use]
    pub fn add_recording_provider(mut self, name: impl Into<String>) -> Self {
        self.config.providers.push(name.into());
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.environment.insert(key.into(), value.into());
        self
    }

    /// Build and start the simulation harness.
    pub async fn build(self) -> Result<SimulationHarness, SimulationError> {
        SimulationHarness::start(self.config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_action(provider: &str) -> Action {
        Action::new(
            "test-ns",
            "test-tenant",
            provider,
            "test-action",
            serde_json::json!({"key": "value"}),
        )
    }

    #[tokio::test]
    async fn single_node_memory_creates_harness() {
        let harness = SimulationHarness::single_node_memory().await.unwrap();

        assert_eq!(harness.node_count(), 1);
        assert!(harness.node(0).is_some());

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn multi_node_memory_creates_cluster() {
        let harness = SimulationHarness::multi_node_memory(3).await.unwrap();

        assert_eq!(harness.node_count(), 3);
        assert!(harness.node(0).is_some());
        assert!(harness.node(1).is_some());
        assert!(harness.node(2).is_some());

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_with_recording_provider() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .build(),
        )
        .await
        .unwrap();

        let action = test_action("email");
        let outcome = harness.dispatch(&action).await.unwrap();

        assert!(matches!(outcome, ActionOutcome::Executed(_)));
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn reset_recordings_clears_calls() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .build(),
        )
        .await
        .unwrap();

        let action = test_action("email");
        harness.dispatch(&action).await.unwrap();
        harness.provider("email").unwrap().assert_called(1);

        harness.reset_recordings();
        harness.provider("email").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_batch_works() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .build(),
        )
        .await
        .unwrap();

        let actions = vec![test_action("email"), test_action("email"), test_action("email")];
        let outcomes = harness.dispatch_batch(&actions).await;

        assert_eq!(outcomes.len(), 3);
        for outcome in outcomes {
            assert!(matches!(outcome.unwrap(), ActionOutcome::Executed(_)));
        }

        harness.provider("email").unwrap().assert_called(3);
        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn dispatch_to_specific_node() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(2)
                .shared_state(true)
                .add_recording_provider("email")
                .build(),
        )
        .await
        .unwrap();

        let action = test_action("email");

        // Dispatch to node 0
        harness.dispatch_to(0, &action).await.unwrap();

        // Dispatch to node 1
        harness.dispatch_to(1, &action).await.unwrap();

        // Provider should have been called twice (once per dispatch)
        harness.provider("email").unwrap().assert_called(2);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn builder_pattern() {
        let harness = SimulationHarnessBuilder::new()
            .nodes(2)
            .shared_state(true)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .build()
            .await
            .unwrap();

        assert_eq!(harness.node_count(), 2);
        assert!(harness.provider("email").is_some());
        assert!(harness.provider("sms").is_some());

        harness.teardown().await.unwrap();
    }
}
