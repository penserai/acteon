use std::collections::HashMap;

use acteon_crypto::{ExposeSecret, SecretString};

use crate::error::VictorOpsError;

/// Default value reported in the `monitoring_tool` field of every
/// alert body. Can be overridden via
/// [`VictorOpsConfig::with_monitoring_tool`].
pub const DEFAULT_MONITORING_TOOL: &str = "acteon";

/// Configuration for the `VictorOps` / Splunk On-Call provider.
///
/// The `VictorOps` REST endpoint integration embeds **two** secrets
/// in the request URL: an organization-level `api_key` and a
/// per-route `routing_key`. Both are held as [`SecretString`] so
/// the plaintexts are zeroized on drop (via the `zeroize` crate,
/// transitively through `secrecy`) and any accidental `Debug`
/// formatting yields `[REDACTED]` instead of the raw values.
///
/// Multiple routing keys can be registered via [`Self::with_route`]
/// so one provider instance can fan alerts out to several
/// `VictorOps` teams based on the dispatch payload's `routing_key`
/// field. A single-route convenience constructor is available for
/// the common case.
#[derive(Clone)]
pub struct VictorOpsConfig {
    /// Organization-level REST integration key (embedded in the
    /// URL path alongside the routing key).
    api_key: SecretString,

    /// Map of logical route name → per-route routing key. The
    /// dispatch payload can select an entry via its `routing_key`
    /// field; if absent, the `default_route_name` is used, and if
    /// there is only one entry it is used implicitly.
    routing_keys: HashMap<String, SecretString>,

    /// Name of the default entry in `routing_keys` used when the
    /// payload omits an explicit `routing_key`.
    default_route_name: Option<String>,

    /// Base URL for the `VictorOps` REST endpoint integration.
    /// Override this to point tests at a mock server.
    api_base_url: String,

    /// Value reported in the `monitoring_tool` field of every alert.
    pub monitoring_tool: String,

    /// Whether to auto-prefix the `entity_id` with
    /// `{namespace}:{tenant}:` before sending it to `VictorOps`.
    ///
    /// **Defaults to `true`.** Leaving this on is the right choice
    /// for deployments where multiple Acteon tenants share a
    /// single `VictorOps` integration key — without it, Tenant A
    /// could resolve Tenant B's alerts by guessing (or observing)
    /// the `entity_id` string. The prefix is applied identically
    /// across trigger / acknowledge / resolve so all three
    /// lifecycle events resolve to the same `VictorOps` incident.
    ///
    /// Set to `false` only if every Acteon namespace/tenant has
    /// its own dedicated `VictorOps` integration key, or you
    /// genuinely need cross-tenant `entity_id` coordination.
    pub scope_entity_ids: bool,
}

impl std::fmt::Debug for VictorOpsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug-format the routing-key map as just the route names;
        // the actual keys are redacted.
        struct RedactedRoutes<'a>(&'a HashMap<String, SecretString>);
        impl std::fmt::Debug for RedactedRoutes<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut map = f.debug_map();
                for name in self.0.keys() {
                    map.entry(name, &"[REDACTED]");
                }
                map.finish()
            }
        }

        f.debug_struct("VictorOpsConfig")
            .field("api_key", &"[REDACTED]")
            .field("routing_keys", &RedactedRoutes(&self.routing_keys))
            .field("default_route_name", &self.default_route_name)
            .field("api_base_url", &self.api_base_url)
            .field("monitoring_tool", &self.monitoring_tool)
            .field("scope_entity_ids", &self.scope_entity_ids)
            .finish()
    }
}

impl VictorOpsConfig {
    /// Create an empty configuration with the given organization
    /// API key. Callers typically chain [`Self::with_route`] to
    /// register at least one routing key before building a
    /// provider.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: SecretString::new(api_key.into()),
            routing_keys: HashMap::new(),
            default_route_name: None,
            api_base_url: "https://alert.victorops.com".to_owned(),
            monitoring_tool: DEFAULT_MONITORING_TOOL.to_owned(),
            scope_entity_ids: true,
        }
    }

    /// Convenience shorthand for the single-route case — registers
    /// one routing key under the given name and marks it as the
    /// default.
    #[must_use]
    pub fn single_route(
        api_key: impl Into<String>,
        route_name: impl Into<String>,
        routing_key: impl Into<String>,
    ) -> Self {
        let route_name = route_name.into();
        let mut config = Self::new(api_key);
        config
            .routing_keys
            .insert(route_name.clone(), SecretString::new(routing_key.into()));
        config.default_route_name = Some(route_name);
        config
    }

    /// Register an additional routing key under a logical name.
    /// The name is what dispatch payloads refer to via their
    /// `routing_key` field.
    #[must_use]
    pub fn with_route(
        mut self,
        route_name: impl Into<String>,
        routing_key: impl Into<String>,
    ) -> Self {
        self.routing_keys
            .insert(route_name.into(), SecretString::new(routing_key.into()));
        self
    }

    /// Set the default route used when the payload omits an
    /// explicit `routing_key`.
    #[must_use]
    pub fn with_default_route(mut self, route_name: impl Into<String>) -> Self {
        self.default_route_name = Some(route_name.into());
        self
    }

    /// Override the API base URL (primarily for tests against a
    /// mock server).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Override the value reported in the alert body's
    /// `monitoring_tool` field.
    #[must_use]
    pub fn with_monitoring_tool(mut self, tool: impl Into<String>) -> Self {
        self.monitoring_tool = tool.into();
        self
    }

    /// Enable or disable automatic `entity_id` scoping. See the
    /// field docs on [`Self::scope_entity_ids`] for the security
    /// implications.
    #[must_use]
    pub fn with_scope_entity_ids(mut self, scope: bool) -> Self {
        self.scope_entity_ids = scope;
        self
    }

    /// Decrypt any `ENC[...]` secrets in the config in place.
    ///
    /// Plain-text values pass through unchanged. Both the
    /// organization `api_key` and every entry in `routing_keys`
    /// are processed so operators can mix-and-match encrypted and
    /// plain values without breaking the provider build.
    #[must_use = "returns the config with decrypted secrets"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, VictorOpsError> {
        self.api_key = acteon_crypto::decrypt_value(self.api_key.expose_secret(), master_key)
            .map_err(|e| {
                VictorOpsError::InvalidPayload(format!("failed to decrypt api_key: {e}"))
            })?;
        let mut decrypted: HashMap<String, SecretString> =
            HashMap::with_capacity(self.routing_keys.len());
        for (name, value) in &self.routing_keys {
            let decrypted_value = acteon_crypto::decrypt_value(value.expose_secret(), master_key)
                .map_err(|e| {
                VictorOpsError::InvalidPayload(format!(
                    "failed to decrypt routing_key '{name}': {e}"
                ))
            })?;
            decrypted.insert(name.clone(), decrypted_value);
        }
        self.routing_keys = decrypted;
        Ok(self)
    }

    /// Return the API base URL (used by the provider to build
    /// request URLs and by tests to assert routing).
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Return the organization API key for URL construction.
    /// Kept `pub(crate)` so the secret cannot leak out of this crate.
    pub(crate) fn api_key(&self) -> &str {
        self.api_key.expose_secret()
    }

    /// Resolve a routing key name to its secret value, honoring
    /// the default-route fallback and the single-entry implicit
    /// fallback.
    ///
    /// # Errors
    ///
    /// - [`VictorOpsError::UnknownRoutingKey`] if the named route is not registered
    /// - [`VictorOpsError::NoDefaultRoutingKey`] if no name was provided and no default is set
    pub(crate) fn resolve_routing_key(&self, name: Option<&str>) -> Result<&str, VictorOpsError> {
        match name {
            Some(n) => self
                .routing_keys
                .get(n)
                .map(|s| s.expose_secret().as_str())
                .ok_or_else(|| VictorOpsError::UnknownRoutingKey(n.to_owned())),
            None => {
                if let Some(default_name) = &self.default_route_name {
                    self.routing_keys
                        .get(default_name.as_str())
                        .map(|s| s.expose_secret().as_str())
                        .ok_or_else(|| VictorOpsError::UnknownRoutingKey(default_name.clone()))
                } else if self.routing_keys.len() == 1 {
                    Ok(self
                        .routing_keys
                        .values()
                        .next()
                        .unwrap()
                        .expose_secret()
                        .as_str())
                } else {
                    Err(VictorOpsError::NoDefaultRoutingKey)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let config = VictorOpsConfig::new("api");
        assert_eq!(config.api_key(), "api");
        assert_eq!(config.api_base_url(), "https://alert.victorops.com");
        assert_eq!(config.monitoring_tool, DEFAULT_MONITORING_TOOL);
        assert!(config.scope_entity_ids);
        assert!(config.routing_keys.is_empty());
        assert!(config.default_route_name.is_none());
    }

    #[test]
    fn single_route_constructor() {
        let config = VictorOpsConfig::single_route("api", "team-ops", "rk-ops");
        assert_eq!(config.resolve_routing_key(None).unwrap(), "rk-ops");
        assert_eq!(
            config.resolve_routing_key(Some("team-ops")).unwrap(),
            "rk-ops"
        );
    }

    #[test]
    fn builder_chain() {
        let config = VictorOpsConfig::new("api")
            .with_route("team-a", "rk-a")
            .with_route("team-b", "rk-b")
            .with_default_route("team-a")
            .with_api_base_url("http://mock")
            .with_monitoring_tool("prometheus")
            .with_scope_entity_ids(false);
        assert_eq!(config.monitoring_tool, "prometheus");
        assert_eq!(config.api_base_url(), "http://mock");
        assert!(!config.scope_entity_ids);
        assert_eq!(config.routing_keys.len(), 2);
        assert_eq!(config.resolve_routing_key(None).unwrap(), "rk-a");
        assert_eq!(config.resolve_routing_key(Some("team-b")).unwrap(), "rk-b");
    }

    #[test]
    fn resolve_explicit() {
        let config = VictorOpsConfig::new("api")
            .with_route("team-a", "rk-a")
            .with_route("team-b", "rk-b");
        assert_eq!(config.resolve_routing_key(Some("team-a")).unwrap(), "rk-a");
    }

    #[test]
    fn resolve_implicit_single() {
        let config = VictorOpsConfig::new("api").with_route("only-team", "rk-only");
        assert_eq!(config.resolve_routing_key(None).unwrap(), "rk-only");
    }

    #[test]
    fn resolve_unknown_route() {
        let config = VictorOpsConfig::single_route("api", "team-a", "rk-a");
        let err = config.resolve_routing_key(Some("team-gone")).unwrap_err();
        assert!(matches!(err, VictorOpsError::UnknownRoutingKey(ref n) if n == "team-gone"));
    }

    #[test]
    fn resolve_no_default_multi() {
        let config = VictorOpsConfig::new("api")
            .with_route("team-a", "rk-a")
            .with_route("team-b", "rk-b");
        let err = config.resolve_routing_key(None).unwrap_err();
        assert!(matches!(err, VictorOpsError::NoDefaultRoutingKey));
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let api_plain = "org-api-key";
        let route_plain = "team-ops-routing-key";
        let api_enc = acteon_crypto::encrypt_value(api_plain, &master_key).unwrap();
        let route_enc = acteon_crypto::encrypt_value(route_plain, &master_key).unwrap();

        let config = VictorOpsConfig::new(api_enc)
            .with_route("team-ops", route_enc)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.api_key(), api_plain);
        assert_eq!(
            config.resolve_routing_key(Some("team-ops")).unwrap(),
            route_plain
        );
    }

    #[test]
    fn decrypt_secrets_passthrough_plaintext() {
        let master_key = test_master_key();
        let config = VictorOpsConfig::single_route("plain-api", "team-ops", "plain-route")
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.api_key(), "plain-api");
        assert_eq!(
            config.resolve_routing_key(Some("team-ops")).unwrap(),
            "plain-route"
        );
    }

    #[test]
    fn decrypt_secrets_invalid_api_key() {
        let master_key = test_master_key();
        let config = VictorOpsConfig::new("ENC[AES256-GCM,data:bad,iv:bad,tag:bad]");
        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, VictorOpsError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_secrets() {
        let config = VictorOpsConfig::new("super-secret-api-placeholder")
            .with_route("team-ops", "super-secret-route-placeholder");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "secrets must be redacted");
        assert!(
            !debug.contains("super-secret-api-placeholder"),
            "api key must not appear in debug output"
        );
        assert!(
            !debug.contains("super-secret-route-placeholder"),
            "routing key must not appear in debug output"
        );
        // Route names themselves should still be visible for debugging.
        assert!(debug.contains("team-ops"));
    }
}
