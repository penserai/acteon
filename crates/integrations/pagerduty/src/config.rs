/// Configuration for the `PagerDuty` provider.
#[derive(Debug, Clone)]
pub struct PagerDutyConfig {
    /// Integration routing key used to authenticate events.
    pub routing_key: String,

    /// Base URL for the `PagerDuty` Events API. Override this for testing
    /// against a mock server.
    pub api_base_url: String,

    /// Default severity when not specified in the event payload.
    pub default_severity: String,

    /// Default source when not specified in the event payload.
    pub default_source: Option<String>,
}

impl PagerDutyConfig {
    /// Create a new configuration with the given routing key.
    ///
    /// Uses the default `PagerDuty` Events API base URL
    /// (`https://events.pagerduty.com`) and severity `"error"`.
    pub fn new(routing_key: impl Into<String>) -> Self {
        Self {
            routing_key: routing_key.into(),
            api_base_url: "https://events.pagerduty.com".to_owned(),
            default_severity: "error".to_owned(),
            default_source: None,
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = PagerDutyConfig::new("test-routing-key");
        assert_eq!(config.routing_key, "test-routing-key");
        assert_eq!(config.api_base_url, "https://events.pagerduty.com");
        assert_eq!(config.default_severity, "error");
        assert!(config.default_source.is_none());
    }

    #[test]
    fn with_api_base_url() {
        let config = PagerDutyConfig::new("key").with_api_base_url("http://localhost:9999");
        assert_eq!(config.api_base_url, "http://localhost:9999");
    }

    #[test]
    fn with_default_severity() {
        let config = PagerDutyConfig::new("key").with_default_severity("critical");
        assert_eq!(config.default_severity, "critical");
    }

    #[test]
    fn with_default_source() {
        let config = PagerDutyConfig::new("key").with_default_source("monitoring");
        assert_eq!(config.default_source.as_deref(), Some("monitoring"));
    }

    #[test]
    fn builder_chain() {
        let config = PagerDutyConfig::new("key")
            .with_api_base_url("http://localhost:1234")
            .with_default_severity("warning")
            .with_default_source("acteon");
        assert_eq!(config.api_base_url, "http://localhost:1234");
        assert_eq!(config.default_severity, "warning");
        assert_eq!(config.default_source.as_deref(), Some("acteon"));
    }
}
