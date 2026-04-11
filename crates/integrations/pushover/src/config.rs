use std::collections::HashMap;

use acteon_crypto::{ExposeSecret, SecretString};

use crate::error::PushoverError;

/// Configuration for the `Pushover` provider.
///
/// Pushover authenticates every request with two secrets: an
/// application-level `app_token` and a per-recipient `user_key`
/// (which can be either an individual user key or a group key).
/// Both live in [`SecretString`] so the plaintexts are zeroized on
/// drop (via the `zeroize` crate, transitively through `secrecy`)
/// and the `Debug` impl redacts them to `[REDACTED]`.
///
/// Multiple user keys can be registered so one provider instance
/// can fan notifications out to several recipients (or groups)
/// based on the dispatch payload's `user_key` field.
#[derive(Clone)]
pub struct PushoverConfig {
    /// Application token (the `T...` key that identifies the
    /// Pushover app registered for Acteon).
    app_token: SecretString,

    /// Map of logical recipient name → user or group key
    /// (`U...` / `G...`). The dispatch payload selects an entry
    /// via its `user_key` field.
    user_keys: HashMap<String, SecretString>,

    /// Name of the default entry in `user_keys` used when the
    /// payload omits an explicit `user_key`.
    default_recipient: Option<String>,

    /// Base URL for the Pushover Messages API. Override this to
    /// point tests at a mock server.
    api_base_url: String,
}

impl std::fmt::Debug for PushoverConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct RedactedRecipients<'a>(&'a HashMap<String, SecretString>);
        impl std::fmt::Debug for RedactedRecipients<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut map = f.debug_map();
                for name in self.0.keys() {
                    map.entry(name, &"[REDACTED]");
                }
                map.finish()
            }
        }
        f.debug_struct("PushoverConfig")
            .field("app_token", &"[REDACTED]")
            .field("user_keys", &RedactedRecipients(&self.user_keys))
            .field("default_recipient", &self.default_recipient)
            .field("api_base_url", &self.api_base_url)
            .finish()
    }
}

impl PushoverConfig {
    /// Create an empty configuration with the given app token.
    /// Callers typically chain [`Self::with_recipient`] to register
    /// at least one user key before building a provider.
    #[must_use]
    pub fn new(app_token: impl Into<String>) -> Self {
        Self {
            app_token: SecretString::new(app_token.into()),
            user_keys: HashMap::new(),
            default_recipient: None,
            api_base_url: "https://api.pushover.net".to_owned(),
        }
    }

    /// Convenience shorthand for the single-recipient case.
    #[must_use]
    pub fn single_recipient(
        app_token: impl Into<String>,
        recipient_name: impl Into<String>,
        user_key: impl Into<String>,
    ) -> Self {
        let recipient_name = recipient_name.into();
        let mut config = Self::new(app_token);
        config
            .user_keys
            .insert(recipient_name.clone(), SecretString::new(user_key.into()));
        config.default_recipient = Some(recipient_name);
        config
    }

    /// Register an additional user or group key under a logical name.
    #[must_use]
    pub fn with_recipient(mut self, name: impl Into<String>, user_key: impl Into<String>) -> Self {
        self.user_keys
            .insert(name.into(), SecretString::new(user_key.into()));
        self
    }

    /// Set the default recipient used when the payload omits an
    /// explicit `user_key`.
    #[must_use]
    pub fn with_default_recipient(mut self, name: impl Into<String>) -> Self {
        self.default_recipient = Some(name.into());
        self
    }

    /// Override the API base URL (primarily for tests).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Decrypt any `ENC[...]` secrets in place.
    #[must_use = "returns the config with decrypted secrets"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, PushoverError> {
        self.app_token = acteon_crypto::decrypt_value(self.app_token.expose_secret(), master_key)
            .map_err(|e| {
            PushoverError::InvalidPayload(format!("failed to decrypt app_token: {e}"))
        })?;
        let mut decrypted: HashMap<String, SecretString> =
            HashMap::with_capacity(self.user_keys.len());
        for (name, value) in &self.user_keys {
            let v =
                acteon_crypto::decrypt_value(value.expose_secret(), master_key).map_err(|e| {
                    PushoverError::InvalidPayload(format!(
                        "failed to decrypt user_key '{name}': {e}"
                    ))
                })?;
            decrypted.insert(name.clone(), v);
        }
        self.user_keys = decrypted;
        Ok(self)
    }

    /// Return the API base URL.
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Return the app token (kept `pub(crate)` so the secret stays
    /// inside this crate).
    pub(crate) fn app_token(&self) -> &str {
        self.app_token.expose_secret()
    }

    /// Resolve a recipient name to its user/group key, honoring the
    /// default-recipient fallback and the single-entry implicit
    /// fallback.
    pub(crate) fn resolve_user_key(&self, name: Option<&str>) -> Result<&str, PushoverError> {
        match name {
            Some(n) => self
                .user_keys
                .get(n)
                .map(|s| s.expose_secret().as_str())
                .ok_or_else(|| PushoverError::UnknownRecipient(n.to_owned())),
            None => {
                if let Some(default_name) = &self.default_recipient {
                    self.user_keys
                        .get(default_name.as_str())
                        .map(|s| s.expose_secret().as_str())
                        .ok_or_else(|| PushoverError::UnknownRecipient(default_name.clone()))
                } else if self.user_keys.len() == 1 {
                    Ok(self
                        .user_keys
                        .values()
                        .next()
                        .unwrap()
                        .expose_secret()
                        .as_str())
                } else {
                    Err(PushoverError::NoDefaultRecipient)
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
        let config = PushoverConfig::new("app");
        assert_eq!(config.app_token(), "app");
        assert_eq!(config.api_base_url(), "https://api.pushover.net");
        assert!(config.user_keys.is_empty());
        assert!(config.default_recipient.is_none());
    }

    #[test]
    fn single_recipient_constructor() {
        let config = PushoverConfig::single_recipient("app", "ops-oncall", "u-ops");
        assert_eq!(config.resolve_user_key(None).unwrap(), "u-ops");
        assert_eq!(
            config.resolve_user_key(Some("ops-oncall")).unwrap(),
            "u-ops"
        );
    }

    #[test]
    fn builder_chain() {
        let config = PushoverConfig::new("app")
            .with_recipient("ops", "u-ops")
            .with_recipient("dev", "u-dev")
            .with_default_recipient("ops")
            .with_api_base_url("http://mock");
        assert_eq!(config.api_base_url(), "http://mock");
        assert_eq!(config.resolve_user_key(None).unwrap(), "u-ops");
        assert_eq!(config.resolve_user_key(Some("dev")).unwrap(), "u-dev");
    }

    #[test]
    fn resolve_unknown_recipient() {
        let config = PushoverConfig::single_recipient("app", "ops", "u-ops");
        let err = config.resolve_user_key(Some("dev")).unwrap_err();
        assert!(matches!(err, PushoverError::UnknownRecipient(ref n) if n == "dev"));
    }

    #[test]
    fn resolve_no_default_multi() {
        let config = PushoverConfig::new("app")
            .with_recipient("ops", "u-ops")
            .with_recipient("dev", "u-dev");
        let err = config.resolve_user_key(None).unwrap_err();
        assert!(matches!(err, PushoverError::NoDefaultRecipient));
    }

    #[test]
    fn resolve_implicit_single() {
        let config = PushoverConfig::new("app").with_recipient("ops", "u-ops");
        assert_eq!(config.resolve_user_key(None).unwrap(), "u-ops");
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let app_plain = "app-token-plain";
        let user_plain = "user-key-plain";
        let app_enc = acteon_crypto::encrypt_value(app_plain, &master_key).unwrap();
        let user_enc = acteon_crypto::encrypt_value(user_plain, &master_key).unwrap();

        let config = PushoverConfig::new(app_enc)
            .with_recipient("ops", user_enc)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.app_token(), app_plain);
        assert_eq!(config.resolve_user_key(Some("ops")).unwrap(), user_plain);
    }

    #[test]
    fn decrypt_secrets_plaintext_passthrough() {
        let master_key = test_master_key();
        let config = PushoverConfig::single_recipient("plain-app", "ops", "plain-user")
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.app_token(), "plain-app");
        assert_eq!(config.resolve_user_key(Some("ops")).unwrap(), "plain-user");
    }

    #[test]
    fn decrypt_secrets_invalid_app_token() {
        let master_key = test_master_key();
        let config = PushoverConfig::new("ENC[AES256-GCM,data:bad,iv:bad,tag:bad]");
        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, PushoverError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_secrets() {
        let config = PushoverConfig::new("super-secret-app-token-placeholder")
            .with_recipient("ops", "super-secret-user-key-placeholder");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-app-token-placeholder"));
        assert!(!debug.contains("super-secret-user-key-placeholder"));
        // Logical names should still be visible for operators.
        assert!(debug.contains("ops"));
    }
}
