//! Server node implementation for simulation testing.

use std::net::SocketAddr;
use std::sync::Arc;

use acteon_core::{Action, ActionOutcome};
use acteon_gateway::{Gateway, GatewayBuilder, GatewayError};
use acteon_provider::DynProvider;
use acteon_rules::Rule;
use acteon_state::{DistributedLock, StateStore};
use acteon_audit::AuditStore;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::error::SimulationError;

/// A single server node in the simulation cluster.
///
/// Each node wraps an Acteon `Gateway` and provides methods for
/// dispatching actions and managing the node lifecycle.
pub struct ServerNode {
    /// Unique identifier for this node.
    pub id: String,
    /// The address this node is listening on.
    pub addr: SocketAddr,
    /// The underlying gateway.
    gateway: Arc<Gateway>,
    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Background task handle (if running HTTP server).
    _handle: Option<JoinHandle<()>>,
}

impl ServerNode {
    /// Create a new server node with the given configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        addr: SocketAddr,
        state: Arc<dyn StateStore>,
        lock: Arc<dyn DistributedLock>,
        rules: Vec<Rule>,
        providers: Vec<Arc<dyn DynProvider>>,
        audit: Option<Arc<dyn AuditStore>>,
        environment: std::collections::HashMap<String, String>,
    ) -> Result<Self, SimulationError> {
        let mut builder = GatewayBuilder::new()
            .state(state)
            .lock(lock)
            .rules(rules);

        for provider in providers {
            builder = builder.provider(provider);
        }

        if let Some(audit_store) = audit {
            builder = builder.audit(audit_store);
        }

        for (key, value) in environment {
            builder = builder.env_var(key, value);
        }

        let gateway = builder.build().map_err(|e| SimulationError::Gateway(e.to_string()))?;

        Ok(Self {
            id: id.into(),
            addr,
            gateway: Arc::new(gateway),
            shutdown_tx: None,
            _handle: None,
        })
    }

    /// Create a builder for a `ServerNode`.
    pub fn builder() -> ServerNodeBuilder {
        ServerNodeBuilder::default()
    }

    /// Get the base URL for this node.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Get a reference to the underlying gateway.
    pub fn gateway(&self) -> &Gateway {
        &self.gateway
    }

    /// Get an `Arc` reference to the underlying gateway.
    pub fn gateway_arc(&self) -> Arc<Gateway> {
        Arc::clone(&self.gateway)
    }

    /// Dispatch an action through this node's gateway.
    pub async fn dispatch(&self, action: Action) -> Result<ActionOutcome, GatewayError> {
        self.gateway.dispatch(action, None).await
    }

    /// Dispatch a batch of actions through this node's gateway.
    pub async fn dispatch_batch(
        &self,
        actions: Vec<Action>,
    ) -> Vec<Result<ActionOutcome, GatewayError>> {
        self.gateway.dispatch_batch(actions, None).await
    }

    /// Gracefully stop this node.
    pub async fn stop(&mut self) -> Result<(), SimulationError> {
        // Signal shutdown if we have a shutdown channel
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for the gateway to complete any pending work
        self.gateway.shutdown().await;

        Ok(())
    }
}

/// Builder for `ServerNode`.
#[derive(Default)]
pub struct ServerNodeBuilder {
    id: Option<String>,
    addr: Option<SocketAddr>,
    state: Option<Arc<dyn StateStore>>,
    lock: Option<Arc<dyn DistributedLock>>,
    rules: Vec<Rule>,
    providers: Vec<Arc<dyn DynProvider>>,
    audit: Option<Arc<dyn AuditStore>>,
    environment: std::collections::HashMap<String, String>,
}

impl ServerNodeBuilder {
    /// Set the node identifier.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the listen address.
    #[must_use]
    pub fn addr(mut self, addr: SocketAddr) -> Self {
        self.addr = Some(addr);
        self
    }

    /// Set the state store.
    #[must_use]
    pub fn state(mut self, state: Arc<dyn StateStore>) -> Self {
        self.state = Some(state);
        self
    }

    /// Set the distributed lock.
    #[must_use]
    pub fn lock(mut self, lock: Arc<dyn DistributedLock>) -> Self {
        self.lock = Some(lock);
        self
    }

    /// Set the rules.
    #[must_use]
    pub fn rules(mut self, rules: Vec<Rule>) -> Self {
        self.rules = rules;
        self
    }

    /// Add a provider.
    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn DynProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Add multiple providers.
    #[must_use]
    pub fn providers(mut self, providers: Vec<Arc<dyn DynProvider>>) -> Self {
        self.providers.extend(providers);
        self
    }

    /// Set the audit store.
    #[must_use]
    pub fn audit(mut self, audit: Arc<dyn AuditStore>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Add an environment variable.
    #[must_use]
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Build the `ServerNode`.
    pub fn build(self) -> Result<ServerNode, SimulationError> {
        let id = self.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let addr = self.addr.ok_or(SimulationError::Configuration(
            "address is required".into(),
        ))?;
        let state = self.state.ok_or(SimulationError::Configuration(
            "state store is required".into(),
        ))?;
        let lock = self.lock.ok_or(SimulationError::Configuration(
            "distributed lock is required".into(),
        ))?;

        ServerNode::new(
            id,
            addr,
            state,
            lock,
            self.rules,
            self.providers,
            self.audit,
            self.environment,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use acteon_core::{Action, ActionOutcome};
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};

    use super::*;
    use crate::provider::RecordingProvider;

    fn test_action() -> Action {
        Action::new(
            "test-ns",
            "test-tenant",
            "test-provider",
            "test-action",
            serde_json::json!({"key": "value"}),
        )
    }

    #[tokio::test]
    async fn node_builder_creates_node() {
        let state = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let provider = Arc::new(RecordingProvider::new("test-provider"));

        let node = ServerNode::builder()
            .id("node-1")
            .addr("127.0.0.1:18001".parse().unwrap())
            .state(state)
            .lock(lock)
            .provider(provider)
            .build()
            .expect("should build node");

        assert_eq!(node.id, "node-1");
        assert_eq!(node.base_url(), "http://127.0.0.1:18001");
    }

    #[tokio::test]
    async fn node_dispatch_executes_action() {
        let state = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());
        let provider = Arc::new(RecordingProvider::new("test-provider"));
        let provider_dyn: Arc<dyn DynProvider> = Arc::clone(&provider) as Arc<dyn DynProvider>;

        let node = ServerNode::builder()
            .id("node-1")
            .addr("127.0.0.1:18002".parse().unwrap())
            .state(state)
            .lock(lock)
            .provider(provider_dyn)
            .build()
            .expect("should build node");

        let outcome = node.dispatch(test_action()).await.expect("should dispatch");

        assert!(matches!(outcome, ActionOutcome::Executed(_)));
        provider.assert_called(1);
    }

    #[tokio::test]
    async fn node_builder_requires_addr() {
        let state = Arc::new(MemoryStateStore::new());
        let lock = Arc::new(MemoryDistributedLock::new());

        let result = ServerNode::builder().state(state).lock(lock).build();

        assert!(result.is_err());
    }
}
