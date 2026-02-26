use serde::{Deserialize, Serialize};

/// Shared base configuration for all Azure providers.
///
/// Contains common settings like tenant ID, service principal credentials,
/// subscription/resource group, and an optional endpoint URL override for
/// local development (e.g. `Azurite`).
#[derive(Clone, Serialize, Deserialize)]
pub struct AzureBaseConfig {
    /// Azure AD tenant ID.
    #[serde(default)]
    pub tenant_id: Option<String>,

    /// Azure AD application (client) ID.
    #[serde(default)]
    pub client_id: Option<String>,

    /// Azure AD client credential (service principal). Redacted in `Debug`.
    #[serde(default)]
    pub client_credential: Option<String>,

    /// Azure subscription ID.
    #[serde(default)]
    pub subscription_id: Option<String>,

    /// Azure resource group name.
    #[serde(default)]
    pub resource_group: Option<String>,

    /// Azure region / location (e.g. `"eastus"`).
    pub location: String,

    /// Optional endpoint URL override for local development (e.g. `Azurite`).
    #[serde(default)]
    pub endpoint_url: Option<String>,
}

impl std::fmt::Debug for AzureBaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureBaseConfig")
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id.as_ref().map(|_| "[REDACTED]"))
            .field(
                "client_credential",
                &self.client_credential.as_ref().map(|_| "[REDACTED]"),
            )
            .field("subscription_id", &self.subscription_id)
            .field("resource_group", &self.resource_group)
            .field("location", &self.location)
            .field("endpoint_url", &self.endpoint_url)
            .finish()
    }
}

impl AzureBaseConfig {
    /// Create a new `AzureBaseConfig` with the given location.
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            tenant_id: None,
            client_id: None,
            client_credential: None,
            subscription_id: None,
            resource_group: None,
            location: location.into(),
            endpoint_url: None,
        }
    }

    /// Set the Azure AD tenant ID.
    #[must_use]
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set the Azure AD application (client) ID.
    #[must_use]
    pub fn with_client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self
    }

    /// Set the Azure AD client credential.
    #[must_use]
    pub fn with_client_credential(mut self, client_credential: impl Into<String>) -> Self {
        self.client_credential = Some(client_credential.into());
        self
    }

    /// Set the Azure subscription ID.
    #[must_use]
    pub fn with_subscription_id(mut self, subscription_id: impl Into<String>) -> Self {
        self.subscription_id = Some(subscription_id.into());
        self
    }

    /// Set the Azure resource group name.
    #[must_use]
    pub fn with_resource_group(mut self, resource_group: impl Into<String>) -> Self {
        self.resource_group = Some(resource_group.into());
        self
    }

    /// Set the endpoint URL override for local development.
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.endpoint_url = Some(endpoint_url.into());
        self
    }
}

impl Default for AzureBaseConfig {
    fn default() -> Self {
        Self {
            tenant_id: None,
            client_id: None,
            client_credential: None,
            subscription_id: None,
            resource_group: None,
            location: "eastus".to_owned(),
            endpoint_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_config_sets_location() {
        let config = AzureBaseConfig::new("westeurope");
        assert_eq!(config.location, "westeurope");
        assert!(config.tenant_id.is_none());
        assert!(config.client_id.is_none());
        assert!(config.endpoint_url.is_none());
    }

    #[test]
    fn builder_chain() {
        let config = AzureBaseConfig::new("eastus2")
            .with_tenant_id("tid-123")
            .with_client_id("cid-456")
            .with_client_credential("cred-789")
            .with_subscription_id("sub-abc")
            .with_resource_group("rg-test")
            .with_endpoint_url("http://127.0.0.1:10000");
        assert_eq!(config.tenant_id.as_deref(), Some("tid-123"));
        assert_eq!(config.client_id.as_deref(), Some("cid-456"));
        assert_eq!(config.client_credential.as_deref(), Some("cred-789"));
        assert_eq!(config.subscription_id.as_deref(), Some("sub-abc"));
        assert_eq!(config.resource_group.as_deref(), Some("rg-test"));
        assert_eq!(
            config.endpoint_url.as_deref(),
            Some("http://127.0.0.1:10000")
        );
    }

    #[test]
    fn debug_redacts_credentials() {
        let config = AzureBaseConfig::new("eastus")
            .with_client_id("my-app-id")
            .with_client_credential("super-private");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("my-app-id"));
        assert!(!debug.contains("super-private"));
    }

    #[test]
    fn serde_roundtrip() {
        let config = AzureBaseConfig::new("northeurope")
            .with_tenant_id("tid-round")
            .with_endpoint_url("http://azurite:10000");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AzureBaseConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.location, "northeurope");
        assert_eq!(deserialized.tenant_id.as_deref(), Some("tid-round"));
        assert_eq!(
            deserialized.endpoint_url.as_deref(),
            Some("http://azurite:10000")
        );
    }

    #[test]
    fn default_config() {
        let config = AzureBaseConfig::default();
        assert_eq!(config.location, "eastus");
        assert!(config.tenant_id.is_none());
        assert!(config.client_id.is_none());
        assert!(config.client_credential.is_none());
        assert!(config.subscription_id.is_none());
        assert!(config.resource_group.is_none());
        assert!(config.endpoint_url.is_none());
    }
}
