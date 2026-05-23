/// Configuration for the Twilio provider.
#[derive(Clone)]
pub struct TwilioConfig {
    /// Twilio Account SID used to authenticate API requests.
    pub account_sid: String,

    /// Twilio Auth Token used for HTTP Basic authentication.
    pub auth_token: String,

    /// Default "From" phone number (E.164 format) when none is specified in
    /// the action payload.
    pub from_number: Option<String>,

    /// Base URL for the Twilio REST API. Override this for testing against a
    /// mock server.
    pub api_base_url: String,
}

impl std::fmt::Debug for TwilioConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TwilioConfig")
            .field("account_sid", &self.account_sid)
            .field("auth_token", &"[REDACTED]")
            .field("from_number", &self.from_number)
            .field("api_base_url", &self.api_base_url)
            .finish()
    }
}

impl TwilioConfig {
    /// Create a new configuration with the given Account SID and Auth Token.
    ///
    /// Uses the default Twilio API base URL (`https://api.twilio.com`).
    pub fn new(account_sid: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            account_sid: account_sid.into(),
            auth_token: auth_token.into(),
            from_number: None,
            api_base_url: "https://api.twilio.com".to_owned(),
        }
    }

    /// Set the default "From" phone number.
    #[must_use]
    pub fn with_from_number(mut self, number: impl Into<String>) -> Self {
        self.from_number = Some(number.into());
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
        let config = TwilioConfig::new("AC123", "token");
        assert_eq!(config.api_base_url, "https://api.twilio.com");
        assert_eq!(config.account_sid, "AC123");
        assert_eq!(config.auth_token, "token");
        assert!(config.from_number.is_none());
    }

    #[test]
    fn with_from_number() {
        let config = TwilioConfig::new("AC123", "token").with_from_number("+15551234567");
        assert_eq!(config.from_number.as_deref(), Some("+15551234567"));
    }

    #[test]
    fn with_custom_api_base_url() {
        let config = TwilioConfig::new("AC123", "token").with_api_base_url("http://localhost:9999");
        assert_eq!(config.api_base_url, "http://localhost:9999");
    }

    #[test]
    fn debug_redacts_auth_token() {
        let config = TwilioConfig::new("AC123", "test-placeholder-value");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "auth_token must be redacted");
        assert!(
            !debug.contains("test-placeholder-value"),
            "auth_token must not appear in debug output"
        );
        assert!(
            debug.contains("AC123"),
            "account_sid should still be visible"
        );
    }
}
