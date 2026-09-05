//! Simulation harness for orchestrating multi-node tests.

use std::collections::HashMap;
use std::sync::Arc;

use acteon_audit_memory::MemoryAuditStore;
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayError;
use acteon_provider::DynProvider;
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_state::DistributedLock;
use acteon_state::StateStore;
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
    state_backend_identity: &'static str,
}

impl SimulationHarness {
    /// Start a simulation cluster with the given configuration.
    pub async fn start(config: SimulationConfig) -> Result<Self, SimulationError> {
        Self::start_with_providers(config, Vec::new()).await
    }

    /// Inject controlled providers while retaining the real backend factory.
    #[allow(clippy::unused_async)] // External-backend awaits are feature-gated.
    pub async fn start_with_providers(
        config: SimulationConfig,
        injected: Vec<Arc<RecordingProvider>>,
    ) -> Result<Self, SimulationError> {
        let port_allocator = PortAllocator::new();

        // Parse rules from YAML
        let rules = Self::parse_rules(&config.rules)?;

        // Create recording providers
        let mut providers: HashMap<String, Arc<RecordingProvider>> = HashMap::new();
        for name in &config.providers {
            providers.insert(name.clone(), Arc::new(RecordingProvider::new(name)));
        }

        for provider in injected {
            providers.insert(provider.name().to_owned(), provider);
        }

        // Convert to DynProvider references
        let provider_refs: Vec<Arc<dyn DynProvider>> = providers
            .values()
            .map(|p| Arc::clone(p) as Arc<dyn DynProvider>)
            .collect();

        if config.nodes == 0 {
            return Err(SimulationError::Configuration(
                "simulation requires at least one node".into(),
            ));
        }
        let (shared_state, shared_lock, state_backend_identity) =
            create_state_backend(&config).await?;

        // All nodes in one simulated deployment share approval signing keys.
        let approval_secret =
            format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()).into_bytes();
        // Create nodes
        let mut nodes = Vec::with_capacity(config.nodes);
        for i in 0..config.nodes {
            let addr = port_allocator
                .allocate()
                .ok_or(SimulationError::PortExhausted)?;

            let state: Arc<dyn StateStore> = shared_state
                .clone()
                .unwrap_or_else(|| Arc::new(MemoryStateStore::new()));

            let audit: Option<Arc<dyn acteon_audit::AuditStore>> = match &config.audit_backend {
                AuditBackendConfig::Memory => Some(Arc::new(MemoryAuditStore::new())),
                AuditBackendConfig::Disabled => None,
            };

            let node = ServerNode::with_executor(
                format!("node-{i}"),
                addr,
                state,
                Arc::clone(&shared_lock),
                rules.clone(),
                provider_refs.clone(),
                audit,
                config.environment.clone(),
                config.state_machines.clone(),
                Some(approval_secret.clone()),
                config.executor_config.clone(),
            )?;

            nodes.push(node);
        }

        Ok(Self {
            nodes,
            providers,
            port_allocator,
            shared_state,
            state_backend_identity,
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

    /// Actual factory-selected state and lock backend, recorded in evaluation manifests.
    #[must_use]
    pub fn state_backend_identity(&self) -> &'static str {
        self.state_backend_identity
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

    /// Dispatch an action to the first node in dry-run mode.
    pub async fn dispatch_dry_run(&self, action: &Action) -> Result<ActionOutcome, GatewayError> {
        self.dispatch_dry_run_to(0, action).await
    }

    /// Dispatch an action to a specific node.
    pub async fn dispatch_to(
        &self,
        node_index: usize,
        action: &Action,
    ) -> Result<ActionOutcome, GatewayError> {
        let node = self
            .nodes
            .get(node_index)
            .ok_or_else(|| GatewayError::Configuration(format!("node {node_index} not found")))?;

        node.dispatch(action.clone()).await
    }

    /// Dispatch an action to a specific node in dry-run mode.
    pub async fn dispatch_dry_run_to(
        &self,
        node_index: usize,
        action: &Action,
    ) -> Result<ActionOutcome, GatewayError> {
        let node = self
            .nodes
            .get(node_index)
            .ok_or_else(|| GatewayError::Configuration(format!("node {node_index} not found")))?;

        node.dispatch_dry_run(action.clone()).await
    }

    /// Dispatch a batch of actions to the first node.
    pub async fn dispatch_batch(
        &self,
        actions: &[Action],
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.dispatch_batch_to(0, actions).await
    }

    /// Dispatch a batch of actions to the first node in dry-run mode.
    pub async fn dispatch_batch_dry_run(
        &self,
        actions: &[Action],
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.dispatch_batch_dry_run_to(0, actions).await
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

    /// Dispatch a batch of actions to a specific node in dry-run mode.
    pub async fn dispatch_batch_dry_run_to(
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

        node.dispatch_batch_dry_run(actions.to_vec()).await
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

type StateComponents = (
    Option<Arc<dyn StateStore>>,
    Arc<dyn DistributedLock>,
    &'static str,
);

/// Construct the selected concrete backend pair; no fallback is permitted.
#[allow(clippy::unused_async)] // External-backend awaits are feature-gated.
async fn create_state_backend(
    config: &SimulationConfig,
) -> Result<StateComponents, SimulationError> {
    let components: StateComponents = match &config.state_backend {
        StateBackendConfig::Memory => (
            config
                .shared_state
                .then(|| Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>),
            Arc::new(MemoryDistributedLock::new()),
            "memory",
        ),
        #[cfg(feature = "postgres")]
        StateBackendConfig::Postgres { url } => {
            use acteon_state_postgres::{
                PostgresConfig, PostgresDistributedLock, PostgresStateStore,
            };
            let postgres = PostgresConfig {
                url: url.clone(),
                table_prefix: format!("sim_{}_", uuid::Uuid::new_v4().simple()),
                ..Default::default()
            };
            let state = PostgresStateStore::new(postgres.clone())
                .await
                .map_err(|e| SimulationError::BackendConnection(e.to_string()))?;
            let lock = PostgresDistributedLock::new(postgres)
                .await
                .map_err(|e| SimulationError::BackendConnection(e.to_string()))?;
            (Some(Arc::new(state)), Arc::new(lock), "postgres")
        }
        #[cfg(feature = "redis")]
        StateBackendConfig::Redis { url, prefix } => {
            use acteon_state_redis::{RedisConfig, RedisDistributedLock, RedisStateStore};
            let redis = RedisConfig {
                url: url.clone(),
                prefix: prefix
                    .clone()
                    .unwrap_or_else(|| format!("sim-{}", uuid::Uuid::new_v4())),
                ..Default::default()
            };
            let state = RedisStateStore::new(&redis)
                .map_err(|e| SimulationError::BackendConnection(e.to_string()))?;
            let lock = RedisDistributedLock::new(&redis)
                .map_err(|e| SimulationError::BackendConnection(e.to_string()))?;
            // Exercise the selected backend during startup; never defer into a memory fallback.
            state
                .get(&acteon_state::StateKey::new(
                    "simulation",
                    "startup",
                    acteon_state::KeyKind::State,
                    "probe",
                ))
                .await
                .map_err(|e| SimulationError::BackendConnection(e.to_string()))?;
            (Some(Arc::new(state)), Arc::new(lock), "redis")
        }
    };
    Ok(components)
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

    /// Add a state machine configuration.
    #[must_use]
    pub fn add_state_machine(mut self, config: acteon_core::StateMachineConfig) -> Self {
        self.config.state_machines.push(config);
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

        let actions = vec![
            test_action("email"),
            test_action("email"),
            test_action("email"),
        ];
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
    #[cfg(feature = "redis")]
    #[tokio::test]
    async fn unavailable_redis_is_an_explicit_startup_failure() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        // A refused connection must be reported, rather than substituted with memory.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            SimulationHarness::start(
                SimulationConfig::builder()
                    .state_backend(StateBackendConfig::Redis {
                        url: format!("redis://127.0.0.1:{port}"),
                        prefix: None,
                    })
                    .build(),
            ),
        )
        .await;
        assert!(matches!(
            result,
            Ok(Err(SimulationError::BackendConnection(_)))
        ));
    }
}
