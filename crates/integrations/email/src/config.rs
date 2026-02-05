use serde::{Deserialize, Serialize};

/// Configuration for the SMTP email provider.
///
/// Holds all settings needed to establish a connection to an SMTP server
/// and send emails. Sensible defaults are provided for common SMTP
/// configurations (port 587, TLS enabled).
///
/// # Examples
///
/// ```
/// use acteon_email::EmailConfig;
///
/// let config = EmailConfig::new("smtp.example.com", "noreply@example.com");
/// assert_eq!(config.smtp_host, "smtp.example.com");
/// assert_eq!(config.smtp_port, 587);
/// assert!(config.tls);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// SMTP server hostname.
    pub smtp_host: String,

    /// SMTP server port. Defaults to 587 (STARTTLS submission port).
    pub smtp_port: u16,

    /// Optional SMTP username for authentication.
    pub username: Option<String>,

    /// Optional SMTP password for authentication.
    pub password: Option<String>,

    /// The `From` address used in outgoing emails.
    pub from_address: String,

    /// Whether to use TLS for the SMTP connection. Defaults to `true`.
    pub tls: bool,
}

impl EmailConfig {
    /// Create a new `EmailConfig` with the given SMTP host and sender address.
    ///
    /// Uses default values for port (587), TLS (enabled), and no authentication.
    ///
    /// # Examples
    ///
    /// ```
    /// use acteon_email::EmailConfig;
    ///
    /// let config = EmailConfig::new("mail.example.com", "sender@example.com");
    /// assert_eq!(config.smtp_port, 587);
    /// assert!(config.tls);
    /// assert!(config.username.is_none());
    /// ```
    pub fn new(smtp_host: impl Into<String>, from_address: impl Into<String>) -> Self {
        Self {
            smtp_host: smtp_host.into(),
            smtp_port: 587,
            username: None,
            password: None,
            from_address: from_address.into(),
            tls: true,
        }
    }

    /// Set SMTP authentication credentials.
    #[must_use]
    pub fn with_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Override the default SMTP port.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.smtp_port = port;
        self
    }

    /// Set whether TLS should be used.
    #[must_use]
    pub fn with_tls(mut self, tls: bool) -> Self {
        self.tls = tls;
        self
    }
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            smtp_host: "localhost".to_owned(),
            smtp_port: 587,
            username: None,
            password: None,
            from_address: "noreply@localhost".to_owned(),
            tls: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let config = EmailConfig::default();
        assert_eq!(config.smtp_host, "localhost");
        assert_eq!(config.smtp_port, 587);
        assert!(config.tls);
        assert!(config.username.is_none());
        assert!(config.password.is_none());
        assert_eq!(config.from_address, "noreply@localhost");
    }

    #[test]
    fn new_config_sets_host_and_from() {
        let config = EmailConfig::new("smtp.gmail.com", "me@gmail.com");
        assert_eq!(config.smtp_host, "smtp.gmail.com");
        assert_eq!(config.from_address, "me@gmail.com");
        assert_eq!(config.smtp_port, 587);
        assert!(config.tls);
    }

    #[test]
    fn with_credentials_sets_auth() {
        let config = EmailConfig::new("smtp.example.com", "sender@example.com")
            .with_credentials("user", "pass");
        assert_eq!(config.username.as_deref(), Some("user"));
        assert_eq!(config.password.as_deref(), Some("pass"));
    }

    #[test]
    fn with_port_overrides_default() {
        let config = EmailConfig::new("smtp.example.com", "sender@example.com").with_port(465);
        assert_eq!(config.smtp_port, 465);
    }

    #[test]
    fn with_tls_can_disable() {
        let config = EmailConfig::new("smtp.example.com", "sender@example.com").with_tls(false);
        assert!(!config.tls);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = EmailConfig::new("smtp.example.com", "test@example.com")
            .with_credentials("user", "secret")
            .with_port(465)
            .with_tls(false);

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EmailConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.smtp_host, "smtp.example.com");
        assert_eq!(deserialized.smtp_port, 465);
        assert_eq!(deserialized.username.as_deref(), Some("user"));
        assert_eq!(deserialized.password.as_deref(), Some("secret"));
        assert_eq!(deserialized.from_address, "test@example.com");
        assert!(!deserialized.tls);
    }
}
