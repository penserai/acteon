/// Configuration for the Slack provider.
#[derive(Clone)]
pub struct SlackConfig {
    /// Bot or user OAuth token used to authenticate API requests.
    pub token: String,

    /// Default channel to post messages to when none is specified in the
    /// action payload.
    pub default_channel: Option<String>,

    /// Base URL for the Slack Web API. Override this for testing against a
    /// mock server.
    pub api_base_url: String,
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackConfig")
            .field("token", &"[REDACTED]")
            .field("default_channel", &self.default_channel)
            .field("api_base_url", &self.api_base_url)
            .finish()
    }
}

impl SlackConfig {
    /// Create a new configuration with the given token.
    ///
    /// Uses the default Slack API base URL (`https://slack.com/api`).
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            default_channel: None,
            api_base_url: "https://slack.com/api".to_owned(),
        }
    }

    /// Set the default channel.
    #[must_use]
    pub fn with_default_channel(mut self, channel: impl Into<String>) -> Self {
        self.default_channel = Some(channel.into());
        self
    }

    /// Override the API base URL (useful for testing).
    #[must_use]
    pub fn with_api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_base_url() {
        let config = SlackConfig::new("xoxb-test-token");
        assert_eq!(config.api_base_url, "https://slack.com/api");
        assert_eq!(config.token, "xoxb-test-token");
        assert!(config.default_channel.is_none());
    }

    #[test]
    fn with_default_channel() {
        let config = SlackConfig::new("xoxb-token").with_default_channel("#general");
        assert_eq!(config.default_channel.as_deref(), Some("#general"));
    }

    #[test]
    fn with_custom_api_base_url() {
        let config = SlackConfig::new("xoxb-token").with_api_base_url("http://localhost:9999/api");
        assert_eq!(config.api_base_url, "http://localhost:9999/api");
    }

    #[test]
    fn debug_redacts_token() {
        let config = SlackConfig::new("xoxb-test-placeholder-value");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "token must be redacted");
        assert!(
            !debug.contains("xoxb-test-placeholder-value"),
            "token must not appear in debug output"
        );
    }
}
