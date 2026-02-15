use std::collections::HashMap;

use acteon_crypto::ExposeSecret;

use crate::error::PagerDutyError;

/// Configuration for the `PagerDuty` provider.
///
/// Supports multiple `PagerDuty` services, each identified by a service ID and
/// mapped to an integration routing key.
#[derive(Clone)]
pub struct PagerDutyConfig {
    /// Map of `PagerDuty` service ID → integration routing key.
    services: HashMap<String, String>,

    /// Fallback service when payload omits `service_id`.
    default_service_id: Option<String>,

    /// Base URL for the `PagerDuty` Events API. Override this for testing
    /// against a mock server.
    pub api_base_url: String,

    /// Default severity when not specified in the event payload.
    pub default_severity: String,

    /// Default source when not specified in the event payload.
    pub default_source: Option<String>,
}

impl std::fmt::Debug for PagerDutyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /// Helper to display service IDs with redacted routing keys.
        struct RedactedServices<'a>(&'a HashMap<String, String>);
        impl std::fmt::Debug for RedactedServices<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut map = f.debug_map();
                for key in self.0.keys() {
                    map.entry(key, &"[REDACTED]");
                }
                map.finish()
            }
        }

        f.debug_struct("PagerDutyConfig")
            .field("services", &RedactedServices(&self.services))
            .field("default_service_id", &self.default_service_id)
            .field("api_base_url", &self.api_base_url)
            .field("default_severity", &self.default_severity)
            .field("default_source", &self.default_source)
            .finish()
    }
}

impl PagerDutyConfig {
    /// Create an empty configuration with no services.
    ///
    /// Uses the default `PagerDuty` Events API base URL
    /// (`https://events.pagerduty.com`) and severity `"error"`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
            default_service_id: None,
            api_base_url: "https://events.pagerduty.com".to_owned(),
            default_severity: "error".to_owned(),
            default_source: None,
        }
    }

    /// Create a configuration with a single service, set as the default.
    ///
    /// This is a convenience shorthand for the common single-service case.
    #[must_use]
    pub fn single_service(service_id: impl Into<String>, routing_key: impl Into<String>) -> Self {
        let service_id = service_id.into();
        let mut services = HashMap::new();
        services.insert(service_id.clone(), routing_key.into());
        Self {
            services,
            default_service_id: Some(service_id),
            api_base_url: "https://events.pagerduty.com".to_owned(),
            default_severity: "error".to_owned(),
            default_source: None,
        }
    }

    /// Add a service to the configuration.
    #[must_use]
    pub fn with_service(
        mut self,
        service_id: impl Into<String>,
        routing_key: impl Into<String>,
    ) -> Self {
        self.services.insert(service_id.into(), routing_key.into());
        self
    }

    /// Set the default service ID used when a payload omits `service_id`.
    #[must_use]
    pub fn with_default_service(mut self, service_id: impl Into<String>) -> Self {
        self.default_service_id = Some(service_id.into());
        self
    }

    /// Override the API base URL (useful for testing).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Set the default severity for trigger events.
    #[must_use]
    pub fn with_default_severity(mut self, severity: impl Into<String>) -> Self {
        self.default_severity = severity.into();
        self
    }

    /// Set the default source for trigger events.
    #[must_use]
    pub fn with_default_source(mut self, source: impl Into<String>) -> Self {
        self.default_source = Some(source.into());
        self
    }

    /// Decrypt any `ENC[...]` routing keys in the services map.
    ///
    /// Plain-text values pass through unchanged.
    #[must_use = "returns the config with decrypted secrets"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, PagerDutyError> {
        for routing_key in self.services.values_mut() {
            routing_key.clone_from(
                acteon_crypto::decrypt_value(routing_key, master_key)
                    .map_err(|e| {
                        PagerDutyError::InvalidPayload(format!(
                            "failed to decrypt routing key: {e}"
                        ))
                    })?
                    .expose_secret(),
            );
        }
        Ok(self)
    }

    /// Resolve a routing key from the services map.
    ///
    /// Resolution order:
    /// 1. Explicit `service_id` → look up in map
    /// 2. No `service_id` + default configured → use default
    /// 3. No `service_id` + no default + exactly 1 service → use it implicitly
    /// 4. Otherwise → error
    pub(crate) fn resolve_routing_key(
        &self,
        service_id: Option<&str>,
    ) -> Result<&str, PagerDutyError> {
        match service_id {
            Some(sid) => self
                .services
                .get(sid)
                .map(String::as_str)
                .ok_or_else(|| PagerDutyError::UnknownService(sid.to_owned())),
            None => {
                if let Some(default_sid) = &self.default_service_id {
                    self.services
                        .get(default_sid.as_str())
                        .map(String::as_str)
                        .ok_or_else(|| PagerDutyError::UnknownService(default_sid.clone()))
                } else if self.services.len() == 1 {
                    // Implicit single-service fallback.
                    Ok(self.services.values().next().unwrap().as_str())
                } else {
                    Err(PagerDutyError::NoDefaultService)
                }
            }
        }
    }
}

impl Default for PagerDutyConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_service_defaults() {
        let config = PagerDutyConfig::single_service("PABC123", "rk-abc");
        assert_eq!(config.api_base_url, "https://events.pagerduty.com");
        assert_eq!(config.default_severity, "error");
        assert!(config.default_source.is_none());
        assert_eq!(config.default_service_id.as_deref(), Some("PABC123"));
        assert_eq!(config.services.get("PABC123").unwrap(), "rk-abc");
    }

    #[test]
    fn new_creates_empty() {
        let config = PagerDutyConfig::new();
        assert!(config.services.is_empty());
        assert!(config.default_service_id.is_none());
    }

    #[test]
    fn with_service_adds_entry() {
        let config = PagerDutyConfig::new().with_service("PSVC1", "key-1");
        assert_eq!(config.services.get("PSVC1").unwrap(), "key-1");
    }

    #[test]
    fn with_default_service() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "key-1")
            .with_default_service("PSVC1");
        assert_eq!(config.default_service_id.as_deref(), Some("PSVC1"));
    }

    #[test]
    fn builder_chain() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "key-1")
            .with_service("PSVC2", "key-2")
            .with_default_service("PSVC1")
            .with_api_base_url("http://localhost:1234")
            .with_default_severity("warning")
            .with_default_source("acteon");
        assert_eq!(config.services.len(), 2);
        assert_eq!(config.api_base_url, "http://localhost:1234");
        assert_eq!(config.default_severity, "warning");
        assert_eq!(config.default_source.as_deref(), Some("acteon"));
    }

    #[test]
    fn resolve_explicit_service() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "key-1")
            .with_service("PSVC2", "key-2");
        assert_eq!(config.resolve_routing_key(Some("PSVC2")).unwrap(), "key-2");
    }

    #[test]
    fn resolve_unknown_service() {
        let config = PagerDutyConfig::new().with_service("PSVC1", "key-1");
        let err = config.resolve_routing_key(Some("PSVC999")).unwrap_err();
        assert!(matches!(err, PagerDutyError::UnknownService(ref s) if s == "PSVC999"));
    }

    #[test]
    fn resolve_uses_default() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "key-1")
            .with_service("PSVC2", "key-2")
            .with_default_service("PSVC1");
        assert_eq!(config.resolve_routing_key(None).unwrap(), "key-1");
    }

    #[test]
    fn resolve_implicit_single() {
        let config = PagerDutyConfig::new().with_service("PSVC1", "key-1");
        assert_eq!(config.resolve_routing_key(None).unwrap(), "key-1");
    }

    #[test]
    fn resolve_no_default_multiple() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "key-1")
            .with_service("PSVC2", "key-2");
        let err = config.resolve_routing_key(None).unwrap_err();
        assert!(matches!(err, PagerDutyError::NoDefaultService));
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let original = "my-routing-key";
        let encrypted = acteon_crypto::encrypt_value(original, &master_key).unwrap();

        let config = PagerDutyConfig::new()
            .with_service("PSVC1", &encrypted)
            .decrypt_secrets(&master_key)
            .unwrap();

        assert_eq!(config.resolve_routing_key(Some("PSVC1")).unwrap(), original);
    }

    #[test]
    fn decrypt_secrets_passthrough() {
        let master_key = test_master_key();
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "plain-key")
            .decrypt_secrets(&master_key)
            .unwrap();

        assert_eq!(
            config.resolve_routing_key(Some("PSVC1")).unwrap(),
            "plain-key"
        );
    }

    #[test]
    fn decrypt_secrets_invalid() {
        let master_key = test_master_key();
        let config =
            PagerDutyConfig::new().with_service("PSVC1", "ENC[AES256-GCM,data:bad,iv:bad,tag:bad]");

        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, PagerDutyError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_routing_keys() {
        let config = PagerDutyConfig::new()
            .with_service("PSVC1", "test-rk-placeholder-1")
            .with_service("PSVC2", "test-rk-placeholder-2");
        let debug = format!("{config:?}");
        assert!(
            debug.contains("[REDACTED]"),
            "routing keys must be redacted"
        );
        assert!(
            !debug.contains("test-rk-placeholder"),
            "routing keys must not appear in debug output"
        );
        assert!(
            debug.contains("PSVC1"),
            "service IDs should still be visible"
        );
        assert!(
            debug.contains("PSVC2"),
            "service IDs should still be visible"
        );
    }
}
