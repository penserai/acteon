//! Configuration types for simulation clusters.

use std::collections::HashMap;

/// Configuration for a simulation cluster.
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    /// Number of nodes in the cluster.
    pub nodes: usize,
    /// Whether nodes share state (requires Redis for multi-node).
    pub shared_state: bool,
    /// State backend configuration.
    pub state_backend: StateBackendConfig,
    /// Audit backend configuration.
    pub audit_backend: AuditBackendConfig,
    /// YAML rule definitions.
    pub rules: Vec<String>,
    /// Provider names to create as `RecordingProvider`.
    pub providers: Vec<String>,
    /// Environment variables available during rule evaluation.
    pub environment: HashMap<String, String>,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            nodes: 1,
            shared_state: false,
            state_backend: StateBackendConfig::Memory,
            audit_backend: AuditBackendConfig::Memory,
            rules: Vec::new(),
            providers: Vec::new(),
            environment: HashMap::new(),
        }
    }
}

impl SimulationConfig {
    /// Create a new builder for `SimulationConfig`.
    pub fn builder() -> SimulationConfigBuilder {
        SimulationConfigBuilder::default()
    }
}

/// Builder for `SimulationConfig`.
#[derive(Debug, Default)]
pub struct SimulationConfigBuilder {
    nodes: Option<usize>,
    shared_state: Option<bool>,
    state_backend: Option<StateBackendConfig>,
    audit_backend: Option<AuditBackendConfig>,
    rules: Vec<String>,
    providers: Vec<String>,
    environment: HashMap<String, String>,
}

impl SimulationConfigBuilder {
    /// Set the number of nodes.
    #[must_use]
    pub fn nodes(mut self, count: usize) -> Self {
        self.nodes = Some(count);
        self
    }

    /// Enable or disable shared state across nodes.
    #[must_use]
    pub fn shared_state(mut self, shared: bool) -> Self {
        self.shared_state = Some(shared);
        self
    }

    /// Set the state backend configuration.
    #[must_use]
    pub fn state_backend(mut self, backend: StateBackendConfig) -> Self {
        self.state_backend = Some(backend);
        self
    }

    /// Set the audit backend configuration.
    #[must_use]
    pub fn audit_backend(mut self, backend: AuditBackendConfig) -> Self {
        self.audit_backend = Some(backend);
        self
    }

    /// Add a YAML rule definition.
    #[must_use]
    pub fn add_rule_yaml(mut self, yaml: impl Into<String>) -> Self {
        self.rules.push(yaml.into());
        self
    }

    /// Add multiple YAML rule definitions.
    #[must_use]
    pub fn add_rules_yaml(mut self, yamls: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.rules.extend(yamls.into_iter().map(Into::into));
        self
    }

    /// Add a recording provider by name.
    #[must_use]
    pub fn add_recording_provider(mut self, name: impl Into<String>) -> Self {
        self.providers.push(name.into());
        self
    }

    /// Add multiple recording providers by name.
    #[must_use]
    pub fn add_recording_providers(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.providers.extend(names.into_iter().map(Into::into));
        self
    }

    /// Add an environment variable for rule evaluation.
    #[must_use]
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Build the `SimulationConfig`.
    #[must_use]
    pub fn build(self) -> SimulationConfig {
        SimulationConfig {
            nodes: self.nodes.unwrap_or(1),
            shared_state: self.shared_state.unwrap_or(false),
            state_backend: self.state_backend.unwrap_or(StateBackendConfig::Memory),
            audit_backend: self.audit_backend.unwrap_or(AuditBackendConfig::Memory),
            rules: self.rules,
            providers: self.providers,
            environment: self.environment,
        }
    }
}

/// State backend configuration.
#[derive(Debug, Clone)]
pub enum StateBackendConfig {
    /// In-memory state (isolated per node unless shared).
    Memory,
    /// Redis-backed state (shared across nodes).
    #[cfg(feature = "redis")]
    Redis {
        /// Redis connection URL.
        url: String,
        /// Optional key prefix for isolation.
        prefix: Option<String>,
    },
}

/// Audit backend configuration.
#[derive(Debug, Clone)]
pub enum AuditBackendConfig {
    /// In-memory audit store.
    Memory,
    /// Disable audit recording.
    Disabled,
}

/// Configuration for a single cluster node.
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Unique node identifier.
    pub node_id: String,
    /// HTTP listen address.
    pub listen_addr: std::net::SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let config = SimulationConfig::builder().build();
        assert_eq!(config.nodes, 1);
        assert!(!config.shared_state);
        assert!(matches!(config.state_backend, StateBackendConfig::Memory));
        assert!(matches!(config.audit_backend, AuditBackendConfig::Memory));
        assert!(config.rules.is_empty());
        assert!(config.providers.is_empty());
    }

    #[test]
    fn builder_with_values() {
        let config = SimulationConfig::builder()
            .nodes(3)
            .shared_state(true)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_rule_yaml("rules: []")
            .env_var("region", "us-east-1")
            .build();

        assert_eq!(config.nodes, 3);
        assert!(config.shared_state);
        assert_eq!(config.providers, vec!["email", "sms"]);
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.environment.get("region"), Some(&"us-east-1".into()));
    }
}
