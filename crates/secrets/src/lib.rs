//! Host-owned secret references and runtime resolution.
//!
//! Durable records carry [`SecretRef`] values, never secret bytes. A host
//! installs one or more [`SecretProvider`] implementations and resolves a
//! reference only at the final effect boundary.

use std::collections::HashMap;
use std::sync::Arc;

use acteon_crypto::{ExposeSecret, SecretString};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const MAX_COMPONENT_BYTES: usize = 256;
const MAX_SECRET_BYTES: usize = 4_096;

/// A durable, non-secret pointer to host-owned credential material.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SecretRef {
    /// Resolver namespace, such as `environment`, `vault`, or `aws-sm`.
    pub provider: String,
    /// Provider-local logical key. This is not secret material.
    pub key: String,
    /// Optional immutable provider version to pin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Host-verified execution context for one resolution. Providers use this to
/// prevent a valid reference from becoming a cross-tenant secret oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretUsage {
    pub namespace: String,
    pub tenant: String,
    pub purpose: String,
    /// Exact non-secret destination or resource receiving the credential.
    pub audience: String,
}

impl SecretUsage {
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        tenant: impl Into<String>,
        purpose: impl Into<String>,
        audience: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            tenant: tenant.into(),
            purpose: purpose.into(),
            audience: audience.into(),
        }
    }
}

impl SecretRef {
    #[must_use]
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
            version: None,
        }
    }

    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn validate(&self) -> Result<(), SecretError> {
        validate_component("provider", &self.provider)?;
        validate_component("key", &self.key)?;
        if let Some(version) = &self.version {
            validate_component("version", version)?;
        }
        Ok(())
    }
}

fn validate_component(name: &'static str, value: &str) -> Result<(), SecretError> {
    if value.is_empty()
        || value.len() > MAX_COMPONENT_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(SecretError::InvalidReference(name));
    }
    Ok(())
}

fn validate_audience(value: &str) -> Result<(), SecretError> {
    if value.is_empty() || value.len() > 2_048 || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(SecretError::InvalidReference("audience"));
    }
    Ok(())
}

/// Resolution failure deliberately carries no provider payload or secret data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("invalid secret reference component: {0}")]
    InvalidReference(&'static str),
    #[error("secret provider is not installed")]
    ProviderNotInstalled,
    #[error("secret reference is not authorized by host configuration")]
    Refused,
    #[error("secret is temporarily unavailable")]
    Unavailable,
}

/// Host adapter for one secret-provider namespace.
#[async_trait]
pub trait SecretProvider: Send + Sync {
    async fn resolve(
        &self,
        reference: &SecretRef,
        usage: &SecretUsage,
    ) -> Result<SecretString, SecretError>;
}

/// Provider registry used at execution boundaries.
#[derive(Default)]
pub struct SecretResolver {
    providers: HashMap<String, Arc<dyn SecretProvider>>,
}

impl SecretResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        name: impl Into<String>,
        provider: Arc<dyn SecretProvider>,
    ) -> Result<(), SecretError> {
        let name = name.into();
        validate_component("provider", &name)?;
        if self.providers.contains_key(&name) {
            return Err(SecretError::Refused);
        }
        self.providers.insert(name, provider);
        Ok(())
    }

    pub async fn resolve(
        &self,
        reference: &SecretRef,
        usage: &SecretUsage,
    ) -> Result<SecretString, SecretError> {
        reference.validate()?;
        validate_component("namespace", &usage.namespace)?;
        validate_component("tenant", &usage.tenant)?;
        validate_component("purpose", &usage.purpose)?;
        validate_audience(&usage.audience)?;
        let provider = self
            .providers
            .get(&reference.provider)
            .ok_or(SecretError::ProviderNotInstalled)?;
        let secret = provider.resolve(reference, usage).await?;
        if secret.expose_secret().is_empty() || secret.expose_secret().len() > MAX_SECRET_BYTES {
            return Err(SecretError::Refused);
        }
        Ok(secret)
    }
}

/// Exact host binding from a logical reference to an environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentBinding {
    pub key: String,
    pub variable: String,
    pub version: Option<String>,
    pub namespace: String,
    pub tenant: String,
    pub purposes: Vec<String>,
    pub audiences: Vec<String>,
}

/// Environment-backed provider. Only explicitly configured logical keys can
/// be resolved; a client cannot use a reference to read arbitrary variables.
pub struct EnvironmentSecretProvider {
    bindings: HashMap<(String, String, String), EnvironmentBinding>,
}

impl EnvironmentSecretProvider {
    pub fn new(
        bindings: impl IntoIterator<Item = EnvironmentBinding>,
    ) -> Result<Self, SecretError> {
        let mut indexed = HashMap::new();
        for binding in bindings {
            validate_component("key", &binding.key)?;
            if binding.variable.is_empty()
                || binding.variable.len() > MAX_COMPONENT_BYTES
                || !binding
                    .variable
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(SecretError::Refused);
            }
            if let Some(version) = &binding.version {
                validate_component("version", version)?;
            }
            validate_component("namespace", &binding.namespace)?;
            validate_component("tenant", &binding.tenant)?;
            if binding.purposes.is_empty()
                || binding
                    .purposes
                    .iter()
                    .any(|purpose| validate_component("purpose", purpose).is_err())
                || binding
                    .purposes
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != binding.purposes.len()
            {
                return Err(SecretError::Refused);
            }
            if binding.audiences.is_empty()
                || binding
                    .audiences
                    .iter()
                    .any(|audience| validate_audience(audience).is_err())
                || binding
                    .audiences
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != binding.audiences.len()
            {
                return Err(SecretError::Refused);
            }
            let identity = (
                binding.namespace.clone(),
                binding.tenant.clone(),
                binding.key.clone(),
            );
            if indexed.insert(identity, binding).is_some() {
                return Err(SecretError::Refused);
            }
        }
        Ok(Self { bindings: indexed })
    }
}

#[async_trait]
impl SecretProvider for EnvironmentSecretProvider {
    async fn resolve(
        &self,
        reference: &SecretRef,
        usage: &SecretUsage,
    ) -> Result<SecretString, SecretError> {
        let binding = self
            .bindings
            .get(&(
                usage.namespace.clone(),
                usage.tenant.clone(),
                reference.key.clone(),
            ))
            .ok_or(SecretError::Refused)?;
        if reference.version.is_some() && reference.version != binding.version {
            return Err(SecretError::Refused);
        }
        if binding.namespace != usage.namespace
            || binding.tenant != usage.tenant
            || !binding.purposes.contains(&usage.purpose)
            || !binding.audiences.contains(&usage.audience)
        {
            return Err(SecretError::Refused);
        }
        let value = std::env::var(&binding.variable).map_err(|_| SecretError::Unavailable)?;
        if value.is_empty() {
            return Err(SecretError::Unavailable);
        }
        Ok(SecretString::new(value.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn environment_provider_requires_an_exact_registered_reference() {
        let provider = EnvironmentSecretProvider::new([
            EnvironmentBinding {
                key: "callbacks/primary".into(),
                variable: "PATH".into(),
                version: Some("v1".into()),
                namespace: "agents".into(),
                tenant: "demo".into(),
                purposes: vec!["a2a.callback.token".into()],
                audiences: vec!["https://callbacks.example/hook".into()],
            },
            EnvironmentBinding {
                key: "callbacks/primary".into(),
                variable: "HOME".into(),
                version: Some("v1".into()),
                namespace: "agents".into(),
                tenant: "other".into(),
                purposes: vec!["a2a.callback.token".into()],
                audiences: vec!["https://other.example/hook".into()],
            },
        ])
        .unwrap();
        let usage = SecretUsage::new(
            "agents",
            "demo",
            "a2a.callback.token",
            "https://callbacks.example/hook",
        );
        provider
            .resolve(
                &SecretRef::new("environment", "callbacks/primary").with_version("v1"),
                &usage,
            )
            .await
            .unwrap();
        assert!(matches!(
            provider
                .resolve(&SecretRef::new("environment", "callbacks/other"), &usage)
                .await,
            Err(SecretError::Refused)
        ));
        let other = provider
            .resolve(
                &SecretRef::new("environment", "callbacks/primary").with_version("v1"),
                &SecretUsage::new(
                    "agents",
                    "other",
                    "a2a.callback.token",
                    "https://other.example/hook",
                ),
            )
            .await
            .unwrap();
        assert_eq!(other.expose_secret(), std::env::var("HOME").unwrap());
        assert!(matches!(
            provider
                .resolve(
                    &SecretRef::new("environment", "callbacks/primary").with_version("v2"),
                    &usage
                )
                .await,
            Err(SecretError::Refused)
        ));
        assert!(matches!(
            provider
                .resolve(
                    &SecretRef::new("environment", "callbacks/primary").with_version("v1"),
                    &SecretUsage::new(
                        "agents",
                        "forbidden",
                        "a2a.callback.token",
                        "https://callbacks.example/hook",
                    )
                )
                .await,
            Err(SecretError::Refused)
        ));
        assert!(matches!(
            provider
                .resolve(
                    &SecretRef::new("environment", "callbacks/primary").with_version("v1"),
                    &SecretUsage::new(
                        "agents",
                        "demo",
                        "a2a.callback.authorization",
                        "https://callbacks.example/hook",
                    )
                )
                .await,
            Err(SecretError::Refused)
        ));
        assert!(matches!(
            provider
                .resolve(
                    &SecretRef::new("environment", "callbacks/primary").with_version("v1"),
                    &SecretUsage::new(
                        "agents",
                        "demo",
                        "a2a.callback.token",
                        "https://attacker.example/collect",
                    )
                )
                .await,
            Err(SecretError::Refused)
        ));
    }
}
