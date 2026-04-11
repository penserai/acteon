use acteon_crypto::{ExposeSecret, SecretString};

use crate::error::WeChatError;

/// Default number of seconds before the server-reported token
/// expiry at which the provider proactively refreshes. Set to 5
/// minutes so a token right at the edge of its TTL does not race
/// an in-flight dispatch.
pub const DEFAULT_TOKEN_REFRESH_BUFFER_SECONDS: u64 = 300;

/// Recipient selection for a `WeChat` Work message.
///
/// `WeChat` lets a single `POST /cgi-bin/message/send` target
/// any combination of users (`touser`), departments (`toparty`),
/// and tags (`totag`). Each field is a `|`-separated string of
/// IDs. The special value `@all` on `touser` broadcasts to every
/// member of the configured `agentid`.
///
/// The provider accepts recipients from the dispatch payload or
/// falls back to a config-level default (see
/// [`WeChatConfig::with_default_recipients`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WeChatRecipients {
    /// `|`-separated list of user IDs, or `@all` for everyone on
    /// the app's visibility list.
    pub touser: Option<String>,
    /// `|`-separated list of department (party) IDs.
    pub toparty: Option<String>,
    /// `|`-separated list of tag IDs.
    pub totag: Option<String>,
}

impl WeChatRecipients {
    /// Whether this recipient selector has at least one address.
    #[must_use]
    pub fn is_populated(&self) -> bool {
        self.touser.is_some() || self.toparty.is_some() || self.totag.is_some()
    }
}

/// Configuration for the `WeChat` Work provider.
#[derive(Clone)]
pub struct WeChatConfig {
    /// Corporation ID (the `corpid` from the `WeChat` Work admin
    /// console). Stored as a [`SecretString`] because while it's
    /// not cryptographically sensitive on its own, it's
    /// enumeration-sensitive and pairs with `corp_secret` to gate
    /// access to the entire tenant.
    corp_id: SecretString,

    /// Corporation secret — the per-app secret from the `WeChat`
    /// Work admin console. This is the credential that pairs
    /// with `corp_id` to mint access tokens.
    corp_secret: SecretString,

    /// Numeric agent ID — identifies which `WeChat` Work app is
    /// sending the message. The admin console assigns one agent
    /// ID per app, and the agent ID determines which users can
    /// receive the message.
    pub agent_id: i64,

    /// Default recipient selector used when the dispatch payload
    /// omits `touser` / `toparty` / `totag`. `None` means the
    /// payload must provide at least one recipient.
    pub default_recipients: Option<WeChatRecipients>,

    /// Default `msgtype` used when the dispatch payload omits it.
    /// Supported values: `"text"`, `"markdown"`, `"textcard"`.
    pub default_msgtype: String,

    /// Base URL for the `WeChat` Work API.
    api_base_url: String,

    /// Buffer window, in seconds, between the proactive token
    /// refresh and the server-reported token expiry. A value of
    /// `300` means the provider refreshes its cached token once
    /// it's within 5 minutes of expiring, rather than waiting
    /// for the server to return `errcode: 42001`.
    pub token_refresh_buffer_seconds: u64,

    /// Whether to set `safe = 1` on outgoing messages. `safe = 1`
    /// marks the message as confidential — recipients cannot
    /// forward, copy, or screenshot it. Defaults to `0`.
    pub safe: i32,

    /// Whether to enable server-side duplicate-message detection.
    /// When `true`, `WeChat` rejects duplicate sends within
    /// `duplicate_check_interval` seconds with a specific error
    /// code. Defaults to `false`.
    pub enable_duplicate_check: bool,

    /// Duplicate-check window in seconds. Only honored when
    /// `enable_duplicate_check` is `true`. Max 1800 per the API.
    pub duplicate_check_interval: Option<u32>,
}

impl std::fmt::Debug for WeChatConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeChatConfig")
            .field("corp_id", &"[REDACTED]")
            .field("corp_secret", &"[REDACTED]")
            .field("agent_id", &self.agent_id)
            .field("default_recipients", &self.default_recipients)
            .field("default_msgtype", &self.default_msgtype)
            .field("api_base_url", &self.api_base_url)
            .field(
                "token_refresh_buffer_seconds",
                &self.token_refresh_buffer_seconds,
            )
            .field("safe", &self.safe)
            .field("enable_duplicate_check", &self.enable_duplicate_check)
            .field("duplicate_check_interval", &self.duplicate_check_interval)
            .finish()
    }
}

impl WeChatConfig {
    /// Create a new configuration.
    ///
    /// `corp_id` and `corp_secret` are the credentials from the
    /// `WeChat` Work admin console. `agent_id` is the numeric ID
    /// of the app within the organization that will send messages.
    #[must_use]
    pub fn new(corp_id: impl Into<String>, corp_secret: impl Into<String>, agent_id: i64) -> Self {
        Self {
            corp_id: SecretString::new(corp_id.into()),
            corp_secret: SecretString::new(corp_secret.into()),
            agent_id,
            default_recipients: None,
            default_msgtype: "text".to_owned(),
            api_base_url: "https://qyapi.weixin.qq.com".to_owned(),
            token_refresh_buffer_seconds: DEFAULT_TOKEN_REFRESH_BUFFER_SECONDS,
            safe: 0,
            enable_duplicate_check: false,
            duplicate_check_interval: None,
        }
    }

    /// Set the default recipient selector.
    #[must_use]
    pub fn with_default_recipients(mut self, recipients: WeChatRecipients) -> Self {
        self.default_recipients = Some(recipients);
        self
    }

    /// Shorthand: set only the default `touser`.
    #[must_use]
    pub fn with_default_touser(mut self, touser: impl Into<String>) -> Self {
        let recipients = self.default_recipients.get_or_insert_default();
        recipients.touser = Some(touser.into());
        self
    }

    /// Shorthand: set only the default `toparty`.
    #[must_use]
    pub fn with_default_toparty(mut self, toparty: impl Into<String>) -> Self {
        let recipients = self.default_recipients.get_or_insert_default();
        recipients.toparty = Some(toparty.into());
        self
    }

    /// Shorthand: set only the default `totag`.
    #[must_use]
    pub fn with_default_totag(mut self, totag: impl Into<String>) -> Self {
        let recipients = self.default_recipients.get_or_insert_default();
        recipients.totag = Some(totag.into());
        self
    }

    /// Override the default `msgtype` (`"text"`, `"markdown"`, or
    /// `"textcard"`).
    #[must_use]
    pub fn with_default_msgtype(mut self, msgtype: impl Into<String>) -> Self {
        self.default_msgtype = msgtype.into();
        self
    }

    /// Override the API base URL (tests only).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Override the token refresh buffer window (seconds).
    #[must_use]
    pub fn with_token_refresh_buffer_seconds(mut self, seconds: u64) -> Self {
        self.token_refresh_buffer_seconds = seconds;
        self
    }

    /// Enable `safe` (confidential) delivery.
    #[must_use]
    pub fn with_safe(mut self, safe: bool) -> Self {
        self.safe = i32::from(safe);
        self
    }

    /// Enable server-side duplicate-check over the given window.
    #[must_use]
    pub fn with_duplicate_check(mut self, interval_seconds: u32) -> Self {
        self.enable_duplicate_check = true;
        self.duplicate_check_interval = Some(interval_seconds);
        self
    }

    /// Decrypt `ENC[...]` `corp_id` and `corp_secret` in place.
    /// Plaintext values pass through unchanged.
    #[must_use = "returns the config with decrypted secrets"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, WeChatError> {
        self.corp_id = acteon_crypto::decrypt_value(self.corp_id.expose_secret(), master_key)
            .map_err(|e| WeChatError::InvalidPayload(format!("failed to decrypt corp_id: {e}")))?;
        self.corp_secret =
            acteon_crypto::decrypt_value(self.corp_secret.expose_secret(), master_key).map_err(
                |e| WeChatError::InvalidPayload(format!("failed to decrypt corp_secret: {e}")),
            )?;
        Ok(self)
    }

    /// Return the API base URL.
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Return the `corp_id`. Kept `pub(crate)` so the secret
    /// stays inside this crate.
    pub(crate) fn corp_id(&self) -> &str {
        self.corp_id.expose_secret()
    }

    /// Return the `corp_secret`. Kept `pub(crate)`.
    pub(crate) fn corp_secret(&self) -> &str {
        self.corp_secret.expose_secret()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let config = WeChatConfig::new("corp", "secret", 1_000_002);
        assert_eq!(config.corp_id(), "corp");
        assert_eq!(config.corp_secret(), "secret");
        assert_eq!(config.agent_id, 1_000_002);
        assert_eq!(config.api_base_url(), "https://qyapi.weixin.qq.com");
        assert_eq!(config.default_msgtype, "text");
        assert_eq!(
            config.token_refresh_buffer_seconds,
            DEFAULT_TOKEN_REFRESH_BUFFER_SECONDS
        );
        assert_eq!(config.safe, 0);
        assert!(!config.enable_duplicate_check);
        assert!(config.default_recipients.is_none());
    }

    #[test]
    fn with_default_touser_builder() {
        let config = WeChatConfig::new("c", "s", 1).with_default_touser("@all");
        let recipients = config.default_recipients.unwrap();
        assert_eq!(recipients.touser.as_deref(), Some("@all"));
        assert!(recipients.toparty.is_none());
        assert!(recipients.totag.is_none());
    }

    #[test]
    fn with_default_recipients_all_three() {
        let config = WeChatConfig::new("c", "s", 1)
            .with_default_touser("u1|u2")
            .with_default_toparty("p1")
            .with_default_totag("t1");
        let r = config.default_recipients.unwrap();
        assert_eq!(r.touser.as_deref(), Some("u1|u2"));
        assert_eq!(r.toparty.as_deref(), Some("p1"));
        assert_eq!(r.totag.as_deref(), Some("t1"));
    }

    #[test]
    fn builder_chain() {
        let config = WeChatConfig::new("c", "s", 42)
            .with_default_msgtype("markdown")
            .with_default_touser("@all")
            .with_api_base_url("http://mock")
            .with_token_refresh_buffer_seconds(60)
            .with_safe(true)
            .with_duplicate_check(1800);
        assert_eq!(config.default_msgtype, "markdown");
        assert_eq!(config.api_base_url(), "http://mock");
        assert_eq!(config.token_refresh_buffer_seconds, 60);
        assert_eq!(config.safe, 1);
        assert!(config.enable_duplicate_check);
        assert_eq!(config.duplicate_check_interval, Some(1800));
    }

    #[test]
    fn recipients_is_populated() {
        let empty = WeChatRecipients::default();
        assert!(!empty.is_populated());

        let touser_only = WeChatRecipients {
            touser: Some("u1".into()),
            ..Default::default()
        };
        assert!(touser_only.is_populated());
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let corp_plain = "corp-plain";
        let secret_plain = "secret-plain";
        let corp_enc = acteon_crypto::encrypt_value(corp_plain, &master_key).unwrap();
        let secret_enc = acteon_crypto::encrypt_value(secret_plain, &master_key).unwrap();

        let config = WeChatConfig::new(corp_enc, secret_enc, 1)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.corp_id(), corp_plain);
        assert_eq!(config.corp_secret(), secret_plain);
    }

    #[test]
    fn decrypt_secrets_plaintext_passthrough() {
        let master_key = test_master_key();
        let config = WeChatConfig::new("plain-corp", "plain-secret", 1)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.corp_id(), "plain-corp");
        assert_eq!(config.corp_secret(), "plain-secret");
    }

    #[test]
    fn decrypt_secrets_invalid_corp_id() {
        let master_key = test_master_key();
        let config =
            WeChatConfig::new("ENC[AES256-GCM,data:bad,iv:bad,tag:bad]", "plain-secret", 1);
        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, WeChatError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_secrets() {
        let config = WeChatConfig::new(
            "super-secret-corp-id-placeholder",
            "super-secret-corp-secret-placeholder",
            1,
        )
        .with_default_touser("@all");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-corp-id-placeholder"));
        assert!(!debug.contains("super-secret-corp-secret-placeholder"));
        // agent_id and recipient selectors are NOT secrets — they
        // should still appear in debug output for operator
        // introspection.
        assert!(debug.contains("agent_id"));
        assert!(debug.contains("@all"));
    }
}
