/// Configuration for the Microsoft Teams provider.
#[derive(Clone)]
pub struct TeamsConfig {
    /// Incoming webhook URL. The URL itself serves as the authentication
    /// credential.
    pub webhook_url: String,
}

impl std::fmt::Debug for TeamsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsConfig")
            .field("webhook_url", &"[REDACTED]")
            .finish()
    }
}

impl TeamsConfig {
    /// Create a new configuration with the given webhook URL.
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_config() {
        let config = TeamsConfig::new("https://outlook.office.com/webhook/abc");
        assert_eq!(config.webhook_url, "https://outlook.office.com/webhook/abc");
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let config = TeamsConfig::new("https://outlook.office.com/webhook/test-placeholder-path");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "webhook_url must be redacted");
        assert!(
            !debug.contains("test-placeholder-path"),
            "webhook_url must not appear in debug output"
        );
    }
}
