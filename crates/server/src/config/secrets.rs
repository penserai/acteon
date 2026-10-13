use std::collections::HashSet;
use std::sync::Arc;

use acteon_secrets::{EnvironmentBinding, EnvironmentSecretProvider, SecretResolver};
use serde::Deserialize;

/// Host-owned secret bindings available to durable execution references.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretCustodyConfig {
    /// Logical keys backed by environment variables. Environment variable
    /// names never enter durable state or client-visible projections.
    #[serde(default)]
    pub environment: Vec<EnvironmentSecretBindingConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSecretBindingConfig {
    pub key: String,
    pub env: String,
    pub namespace: String,
    pub tenant: String,
    pub purposes: Vec<String>,
    /// Exact callback URLs authorized to receive the secret.
    pub audiences: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
}

impl SecretCustodyConfig {
    pub fn build_resolver(&self) -> Result<Arc<SecretResolver>, String> {
        let mut keys = HashSet::new();
        let mut envs = HashSet::new();
        let mut bindings = Vec::with_capacity(self.environment.len());
        for binding in &self.environment {
            if !keys.insert((
                binding.namespace.clone(),
                binding.tenant.clone(),
                binding.key.clone(),
            )) {
                return Err(format!(
                    "duplicate scoped environment secret key: {}/{}/{}",
                    binding.namespace, binding.tenant, binding.key
                ));
            }
            if !envs.insert(binding.env.clone()) {
                return Err(format!(
                    "environment secret variable reused: {}",
                    binding.env
                ));
            }
            bindings.push(EnvironmentBinding {
                key: binding.key.clone(),
                variable: binding.env.clone(),
                version: binding.version.clone(),
                namespace: binding.namespace.clone(),
                tenant: binding.tenant.clone(),
                purposes: binding.purposes.clone(),
                audiences: binding.audiences.clone(),
            });
        }
        let provider =
            EnvironmentSecretProvider::new(bindings).map_err(|error| error.to_string())?;
        let mut resolver = SecretResolver::new();
        resolver
            .register("environment", Arc::new(provider))
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(resolver))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_logical_keys() {
        let config = SecretCustodyConfig {
            environment: vec![
                EnvironmentSecretBindingConfig {
                    key: "callbacks/main".into(),
                    env: "CALLBACK_ONE".into(),
                    namespace: "agents".into(),
                    tenant: "demo".into(),
                    purposes: vec!["a2a.callback.token".into()],
                    audiences: vec!["https://callbacks.example/hook".into()],
                    version: None,
                },
                EnvironmentSecretBindingConfig {
                    key: "callbacks/main".into(),
                    env: "CALLBACK_TWO".into(),
                    namespace: "agents".into(),
                    tenant: "demo".into(),
                    purposes: vec!["a2a.callback.token".into()],
                    audiences: vec!["https://callbacks.example/hook".into()],
                    version: None,
                },
            ],
        };
        assert!(config.build_resolver().is_err());
    }
}
