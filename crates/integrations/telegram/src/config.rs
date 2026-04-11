use std::collections::HashMap;

use acteon_crypto::{ExposeSecret, SecretString};

use crate::error::TelegramError;

/// Default maximum length for the `text` field sent to Telegram,
/// in UTF-8 bytes. The actual API cap is 4096 **UTF-16 code units**,
/// which for mostly-ASCII text works out to ~4096 bytes; for
/// non-BMP characters (emoji above U+FFFF, CJK surrogate pairs)
/// the per-byte cap is stricter than necessary but never
/// over-estimates. Override via
/// [`TelegramConfig::with_text_max_bytes`] if you know your
/// traffic is all-ASCII and want the full 4096-byte runway.
pub const DEFAULT_TEXT_MAX_BYTES: usize = 4096;

/// Configuration for the Telegram Bot provider.
///
/// Telegram bots authenticate via a single token embedded in the
/// URL path (`https://api.telegram.org/bot{token}/sendMessage`).
/// The token lives in [`SecretString`] so its plaintext is
/// zeroized on drop (via the `zeroize` crate, transitively through
/// `secrecy`) and the `Debug` impl redacts it.
///
/// Chat IDs, by contrast, are **not** secrets — they're routing
/// identifiers that appear in every message payload and can be
/// recovered from the bot's own `getUpdates` response. They're
/// stored as plain `String` values in a `HashMap` keyed by a
/// logical name the dispatch payload uses to pick between them.
#[derive(Clone)]
pub struct TelegramConfig {
    /// Bot token (`{bot_id}:{auth-string}`) — the secret that
    /// authenticates every API call.
    bot_token: SecretString,

    /// Map of logical chat name → Telegram `chat_id`.
    ///
    /// Chat IDs can be numeric (`-1001234567890`) or string
    /// `@channelusername` handles. Both forms are passed through
    /// verbatim to the API.
    chats: HashMap<String, String>,

    /// Name of the default chat used when the payload omits
    /// `chat`.
    default_chat_name: Option<String>,

    /// Base URL for the Bot API. Override for tests.
    api_base_url: String,

    /// Default `parse_mode` applied to outgoing messages when the
    /// payload omits it. `None` means Telegram's default (plain
    /// text) is used.
    pub default_parse_mode: Option<String>,

    /// Client-side text truncation cap, in bytes.
    pub text_max_bytes: usize,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("bot_token", &"[REDACTED]")
            .field("chats", &self.chats)
            .field("default_chat_name", &self.default_chat_name)
            .field("api_base_url", &self.api_base_url)
            .field("default_parse_mode", &self.default_parse_mode)
            .field("text_max_bytes", &self.text_max_bytes)
            .finish()
    }
}

impl TelegramConfig {
    /// Create an empty configuration with the given bot token.
    /// Callers typically chain [`Self::with_chat`] to register
    /// at least one chat id before building a provider.
    #[must_use]
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: SecretString::new(bot_token.into()),
            chats: HashMap::new(),
            default_chat_name: None,
            api_base_url: "https://api.telegram.org".to_owned(),
            default_parse_mode: None,
            text_max_bytes: DEFAULT_TEXT_MAX_BYTES,
        }
    }

    /// Convenience shorthand for the single-chat case — registers
    /// one chat id under the given name and marks it as the
    /// default.
    #[must_use]
    pub fn single_chat(
        bot_token: impl Into<String>,
        chat_name: impl Into<String>,
        chat_id: impl Into<String>,
    ) -> Self {
        let chat_name = chat_name.into();
        let mut config = Self::new(bot_token);
        config.chats.insert(chat_name.clone(), chat_id.into());
        config.default_chat_name = Some(chat_name);
        config
    }

    /// Register an additional chat under a logical name.
    #[must_use]
    pub fn with_chat(mut self, name: impl Into<String>, chat_id: impl Into<String>) -> Self {
        self.chats.insert(name.into(), chat_id.into());
        self
    }

    /// Set the default chat used when the payload omits `chat`.
    #[must_use]
    pub fn with_default_chat(mut self, name: impl Into<String>) -> Self {
        self.default_chat_name = Some(name.into());
        self
    }

    /// Override the API base URL (tests only).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Set the default `parse_mode` (`"HTML"`, `"Markdown"`, or
    /// `"MarkdownV2"`).
    #[must_use]
    pub fn with_default_parse_mode(mut self, mode: impl Into<String>) -> Self {
        self.default_parse_mode = Some(mode.into());
        self
    }

    /// Override the client-side `text` truncation cap.
    #[must_use]
    pub fn with_text_max_bytes(mut self, bytes: usize) -> Self {
        self.text_max_bytes = bytes;
        self
    }

    /// Decrypt any `ENC[...]` bot token in place. Plaintext tokens
    /// pass through unchanged.
    #[must_use = "returns the config with the decrypted bot token"]
    pub fn decrypt_secrets(
        mut self,
        master_key: &acteon_crypto::MasterKey,
    ) -> Result<Self, TelegramError> {
        self.bot_token = acteon_crypto::decrypt_value(self.bot_token.expose_secret(), master_key)
            .map_err(|e| {
            TelegramError::InvalidPayload(format!("failed to decrypt bot_token: {e}"))
        })?;
        Ok(self)
    }

    /// Return the API base URL.
    #[must_use]
    pub fn api_base_url(&self) -> &str {
        &self.api_base_url
    }

    /// Return the bot token (kept `pub(crate)` so the secret stays
    /// inside this crate).
    pub(crate) fn bot_token(&self) -> &str {
        self.bot_token.expose_secret()
    }

    /// Resolve a chat name to its chat id, honoring the default
    /// and single-entry implicit fallbacks.
    pub(crate) fn resolve_chat_id(&self, name: Option<&str>) -> Result<&str, TelegramError> {
        match name {
            Some(n) => self
                .chats
                .get(n)
                .map(String::as_str)
                .ok_or_else(|| TelegramError::UnknownChat(n.to_owned())),
            None => {
                if let Some(default_name) = &self.default_chat_name {
                    self.chats
                        .get(default_name.as_str())
                        .map(String::as_str)
                        .ok_or_else(|| TelegramError::UnknownChat(default_name.clone()))
                } else if self.chats.len() == 1 {
                    Ok(self.chats.values().next().unwrap())
                } else {
                    Err(TelegramError::NoDefaultChat)
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
        let config = TelegramConfig::new("tok");
        assert_eq!(config.bot_token(), "tok");
        assert_eq!(config.api_base_url(), "https://api.telegram.org");
        assert!(config.chats.is_empty());
        assert!(config.default_chat_name.is_none());
        assert!(config.default_parse_mode.is_none());
        assert_eq!(config.text_max_bytes, DEFAULT_TEXT_MAX_BYTES);
    }

    #[test]
    fn single_chat_constructor() {
        let config = TelegramConfig::single_chat("tok", "ops", "-1001234");
        assert_eq!(config.resolve_chat_id(None).unwrap(), "-1001234");
        assert_eq!(config.resolve_chat_id(Some("ops")).unwrap(), "-1001234");
    }

    #[test]
    fn builder_chain() {
        let config = TelegramConfig::new("tok")
            .with_chat("ops", "-1001234")
            .with_chat("dev", "@devchannel")
            .with_default_chat("ops")
            .with_api_base_url("http://mock")
            .with_default_parse_mode("HTML")
            .with_text_max_bytes(1024);
        assert_eq!(config.default_parse_mode.as_deref(), Some("HTML"));
        assert_eq!(config.text_max_bytes, 1024);
        assert_eq!(config.resolve_chat_id(None).unwrap(), "-1001234");
        assert_eq!(config.resolve_chat_id(Some("dev")).unwrap(), "@devchannel");
    }

    #[test]
    fn resolve_explicit() {
        let config = TelegramConfig::new("tok")
            .with_chat("ops", "-1")
            .with_chat("dev", "@dev");
        assert_eq!(config.resolve_chat_id(Some("dev")).unwrap(), "@dev");
    }

    #[test]
    fn resolve_implicit_single() {
        let config = TelegramConfig::new("tok").with_chat("only", "-1");
        assert_eq!(config.resolve_chat_id(None).unwrap(), "-1");
    }

    #[test]
    fn resolve_unknown() {
        let config = TelegramConfig::single_chat("tok", "ops", "-1");
        let err = config.resolve_chat_id(Some("dev")).unwrap_err();
        assert!(matches!(err, TelegramError::UnknownChat(ref n) if n == "dev"));
    }

    #[test]
    fn resolve_no_default_multi() {
        let config = TelegramConfig::new("tok")
            .with_chat("ops", "-1")
            .with_chat("dev", "-2");
        let err = config.resolve_chat_id(None).unwrap_err();
        assert!(matches!(err, TelegramError::NoDefaultChat));
    }

    fn test_master_key() -> acteon_crypto::MasterKey {
        acteon_crypto::parse_master_key(&"42".repeat(32)).unwrap()
    }

    #[test]
    fn decrypt_secrets_roundtrip() {
        let master_key = test_master_key();
        let plain = "123456:ABC-DEF";
        let encrypted = acteon_crypto::encrypt_value(plain, &master_key).unwrap();

        let config = TelegramConfig::new(encrypted)
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.bot_token(), plain);
    }

    #[test]
    fn decrypt_secrets_plaintext_passthrough() {
        let master_key = test_master_key();
        let config = TelegramConfig::new("plain-token")
            .decrypt_secrets(&master_key)
            .unwrap();
        assert_eq!(config.bot_token(), "plain-token");
    }

    #[test]
    fn decrypt_secrets_invalid() {
        let master_key = test_master_key();
        let config = TelegramConfig::new("ENC[AES256-GCM,data:bad,iv:bad,tag:bad]");
        let err = config.decrypt_secrets(&master_key).unwrap_err();
        assert!(matches!(err, TelegramError::InvalidPayload(_)));
    }

    #[test]
    fn debug_redacts_bot_token() {
        let config =
            TelegramConfig::single_chat("super-secret-bot-token-placeholder", "ops", "-1001234");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-bot-token-placeholder"));
        // Chat IDs and names are NOT secrets — they should still
        // appear in debug output.
        assert!(debug.contains("ops"));
        assert!(debug.contains("-1001234"));
    }
}
