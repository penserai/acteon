use acteon_crypto::ExposeSecret;

use crate::error::OpsGenieError;

/// `OpsGenie` data residency region.
///
/// `OpsGenie` runs two physically separate API endpoints: one in the US
/// and one in the EU. Accounts are pinned to one region at provisioning
/// time and API keys only work against their home region, so picking
/// the wrong one surfaces as a 401/403 rather than a silent misdelivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpsGenieRegion {
    /// `api.opsgenie.com` — default, and the region for most accounts.
    #[default]
    Us,
    /// `api.eu.opsgenie.com` — for EU-region `OpsGenie` accounts.
    Eu,
}

impl OpsGenieRegion {
    /// Base URL for this region's Alert API v2 endpoint.
    #[must_use]
    pub const fn base_url(&self) -> &'static str {
        match self {
            Self::Us => "https://api.opsgenie.com",
            Self::Eu => "https://api.eu.opsgenie.com",
        }
    }
}

/// Configuration for the `OpsGenie` provider.
#[derive(Clone)]
pub struct OpsGenieConfig {
    /// API integration key (the `GenieKey` that authenticates writes
    /// against the Alert API).
    api_key: String,

    /// `OpsGenie` region the account lives in.
    region: OpsGenieRegion,

    /// Base URL override — primarily for testing against a mock
    /// server. When `None`, the URL is derived from `region`.
    api_base_url_override: Option<String>,

    /// Default team responder used when the payload omits one.
    pub default_team: Option<String>,

    /// Default alert priority (`P1`..=`P5`). Defaults to `P3`.
    pub default_priority: String,

    /// Default `source` field used when the payload omits one.
    pub default_source: Option<String>,
}

impl std::fmt::Debug for OpsGenieConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpsGenieConfig")
            .field("api_key", &"[REDACTED]")
            .field("region", &self.region)
            .field("api_base_url_override", &self.api_base_url_override)
            .field("default_team", &self.default_team)
            .field("default_priority", &self.default_priority)
            .field("default_source", &self.default_source)
            .finish()
    }
}

impl OpsGenieConfig {
    /// Create a new configuration with the given API key.
    ///
    /// Defaults to the US region, priority `P3`, and no default
    /// responder / source. Callers typically chain `with_*` builders
    /// to customize the rest.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            region: OpsGenieRegion::Us,
            api_base_url_override: None,
            default_team: None,
            default_priority: "P3".to_owned(),
            default_source: None,
        }
    }

    /// Set the `OpsGenie` region for this configuration.
    #[must_use]
    pub fn with_region(mut self, region: OpsGenieRegion) -> Self {
        self.region = region;
        self
    }

    /// Override the API base URL (useful for testing against a mock
    /// server). When set, this takes precedence over the region's
    /// built-in base URL.
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url_override = Some(url.into());
        self
    }

    /// Set the default team responder used when a payload omits one.
    #[must_use]
    pub fn with_default_team(mut self, team: impl Into<String>) -> Self {
        self.default_team = Some(team.into());
        self
    }

    /// Set the default alert priority (`P1`..=`P5`).
    #[must_use]
    pub fn with_default_priority(mut self, priority: impl Into<String>) -> Self {
        self.default_priority = priority.into();
        self
    }

    /// Set the default alert source.
    #[must_use]
    pub fn with_default_source(mut self, source: impl Into<String>) -> Self {
        self.default_source = Some(source.into());
        self
    }

    /// Decrypt an `ENC[...]` API key in place.
    ///
    /// Plain-text keys pass through unchanged.
    #[must_use = "returns the config with the decrypted API key"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, OpsGenieError> {
        let decrypted = acteon_crypto::decrypt_value(&self.api_key, master_key)
            .map_err(|e| OpsGenieError::InvalidPayload(format!("failed to decrypt api_key: {e}")))?;
        decrypted.expose_secret().clone_into(&mut self.api_key);
        Ok(self)
    }

    /// Return the effective base URL for API requests.
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        self.api_base_url_override
            .as_deref()
            .unwrap_or_else(|| self.region.base_url())
    }

    /// Return the API key for constructing the `Authorization` header.
    /// Kept `pub(crate)` so callers outside the crate cannot lift the
    /// secret out of the config struct.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Return the configured region.
    #[must_use]
    pub fn region(&self) -> OpsGenieRegion {
        self.region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let config = OpsGenieConfig::new("test-api-key");
        assert_eq!(config.api_key(), "test-api-key");
        assert_eq!(config.region(), OpsGenieRegion::Us);
        assert_eq!(config.default_priority, "P3");
        assert!(config.default_team.is_none());
        assert!(config.default_source.is_none());
    }

    #[test]
    fn region_base_urls() {
        assert_eq!(OpsGenieRegion::Us.base_url(), "https://api.opsgenie.com");
        assert_eq!(OpsGenieRegion::Eu.base_url(), "https://api.eu.opsgenie.com");
    }

    #[test]
    fn with_region_switches_base_url() {
        let config = OpsGenieConfig::new("k").with_region(OpsGenieRegion::Eu);
        assert_eq!(config.api_base_url(), "https://api.eu.opsgenie.com");
    }

    #[test]
    fn api_base_url_override_wins() {
        let config = OpsGenieConfig::new("k")
            .with_region(OpsGenieRegion::Eu)
            .with_api_base_url("http://localhost:4242");
        assert_eq!(config.api_base_url(), "http://localhost:4242");
    }

    #[test]
    fn builder_chain() {
        let config = OpsGenieConfig::new("k")
            .with_region(OpsGenieRegion::Eu)
            .with_default_team("ops-team")
            .with_default_priority("P1")
            .with_default_source("prometheus")
            .with_api_base_url("http://mock");
        assert_eq!(config.default_team.as_deref(), Some("ops-team"));
        assert_eq!(config.default_priority, "P1");
        assert_eq!(config.default_source.as_deref(), Some("prometheus"));
        assert_eq!(config.api_base_url(), "http://mock");
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let original = "my-genie-key";
        let encrypted = acteon_crypto::encrypt_value(original, &master_key).unwrap();

        let config = OpsGenieConfig::new(encrypted)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.api_key(), original);
    }

    #[test]
    fn decrypt_secrets_plaintext_passthrough() {
        let master_key = test_master_key();
        let config = OpsGenieConfig::new("plain-key")
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.api_key(), "plain-key");
    }

    #[test]
    fn decrypt_secrets_invalid_ciphertext() {
        let master_key = test_master_key();
        let config = OpsGenieConfig::new("ENC[AES256-GCM,data:bad,iv:bad,tag:bad]");
        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, OpsGenieError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_api_key() {
        let config = OpsGenieConfig::new("super-secret-placeholder-value");
        let debug = format!("{config:?}");
        assert!(
            debug.contains("[REDACTED]"),
            "api_key must be redacted in Debug output"
        );
        assert!(
            !debug.contains("super-secret-placeholder-value"),
            "raw api_key must not appear in Debug output"
        );
    }
}
