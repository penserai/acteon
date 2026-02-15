/// Configuration for the Discord provider.
#[derive(Clone)]
pub struct DiscordConfig {
    /// Discord webhook URL.
    pub webhook_url: String,

    /// Whether to append `?wait=true` to the webhook URL, causing Discord to
    /// return the created message object instead of 204 No Content.
    pub wait: bool,

    /// Default username to display in Discord messages.
    pub default_username: Option<String>,

    /// Default avatar URL to display in Discord messages.
    pub default_avatar_url: Option<String>,
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("webhook_url", &"[REDACTED]")
            .field("wait", &self.wait)
            .field("default_username", &self.default_username)
            .field("default_avatar_url", &self.default_avatar_url)
            .finish()
    }
}

impl DiscordConfig {
    /// Create a new configuration with the given webhook URL.
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            wait: false,
            default_username: None,
            default_avatar_url: None,
        }
    }

    /// Enable `?wait=true` on webhook requests.
    #[must_use]
    pub fn with_wait(mut self, wait: bool) -> Self {
        self.wait = wait;
        self
    }

    /// Set the default username for messages.
    #[must_use]
    pub fn with_default_username(mut self, username: impl Into<String>) -> Self {
        self.default_username = Some(username.into());
        self
    }

    /// Set the default avatar URL for messages.
    #[must_use]
    pub fn with_default_avatar_url(mut self, url: impl Into<String>) -> Self {
        self.default_avatar_url = Some(url.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc");
        assert_eq!(
            config.webhook_url,
            "https://discord.com/api/webhooks/123/abc"
        );
        assert!(!config.wait);
        assert!(config.default_username.is_none());
        assert!(config.default_avatar_url.is_none());
    }

    #[test]
    fn with_all_options() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc")
            .with_wait(true)
            .with_default_username("Acteon Bot")
            .with_default_avatar_url("https://example.com/avatar.png");
        assert!(config.wait);
        assert_eq!(config.default_username.as_deref(), Some("Acteon Bot"));
        assert_eq!(
            config.default_avatar_url.as_deref(),
            Some("https://example.com/avatar.png")
        );
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let config = DiscordConfig::new("https://discord.com/api/webhooks/123/test-placeholder");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "webhook_url must be redacted");
        assert!(
            !debug.contains("test-placeholder"),
            "webhook_url must not appear in debug output"
        );
    }
}
