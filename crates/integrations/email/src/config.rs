use serde::{Deserialize, Serialize};

/// SMTP-specific configuration settings.
///
/// Holds all settings needed to establish a connection to an SMTP server.
#[derive(Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    /// SMTP server hostname.
    pub smtp_host: String,

    /// SMTP server port. Defaults to 587 (STARTTLS submission port).
    pub smtp_port: u16,

    /// Optional SMTP username for authentication.
    pub username: Option<String>,

    /// Optional SMTP password for authentication.
    pub password: Option<String>,

    /// Whether to use TLS for the SMTP connection. Defaults to `true`.
    pub tls: bool,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("tls", &self.tls)
            .finish()
    }
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            smtp_host: "localhost".to_owned(),
            smtp_port: 587,
            username: None,
            password: None,
            tls: true,
        }
    }
}

/// Full email provider configuration.
///
/// Wraps a `from_address` and a backend-specific config. The backend
/// defaults to SMTP for backward compatibility.
///
/// # Examples
///
/// ```
/// use acteon_email::EmailConfig;
///
/// // SMTP (default)
/// let config = EmailConfig::new("smtp.example.com", "noreply@example.com");
/// assert_eq!(config.smtp_host(), Some("smtp.example.com"));
/// assert!(config.tls());
/// ```
#[derive(Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    /// The `From` address used in outgoing emails.
    pub from_address: String,

    /// Backend selection: `"smtp"` (default) or `"ses"`.
    #[serde(default = "default_backend")]
    pub backend: String,

    // ---- SMTP fields (backward-compatible) ----
    /// SMTP server hostname.
    #[serde(default)]
    pub smtp_host: String,

    /// SMTP server port. Defaults to 587.
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,

    /// Optional SMTP username for authentication.
    pub username: Option<String>,

    /// Optional SMTP password for authentication.
    pub password: Option<String>,

    /// Whether to use TLS for SMTP. Defaults to `true`.
    #[serde(default = "default_tls")]
    pub tls: bool,

    // ---- SES fields ----
    /// AWS region for SES backend.
    #[serde(default)]
    pub aws_region: Option<String>,

    /// Optional IAM role ARN for SES cross-account access.
    #[serde(default)]
    pub aws_role_arn: Option<String>,

    /// Optional AWS endpoint URL override for SES (e.g. `LocalStack`).
    #[serde(default)]
    pub aws_endpoint_url: Option<String>,

    /// Optional SES configuration set name for email tracking.
    #[serde(default)]
    pub ses_configuration_set: Option<String>,

    /// Optional STS session name for SES assume-role.
    #[serde(default)]
    pub aws_session_name: Option<String>,

    /// Optional external ID for SES cross-account trust policies.
    #[serde(default)]
    pub aws_external_id: Option<String>,
}

fn default_backend() -> String {
    "smtp".to_owned()
}

fn default_smtp_port() -> u16 {
    587
}

fn default_tls() -> bool {
    true
}

impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("from_address", &self.from_address)
            .field("backend", &self.backend)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("tls", &self.tls)
            .field("aws_region", &self.aws_region)
            .field(
                "aws_role_arn",
                &self.aws_role_arn.as_ref().map(|_| "[REDACTED]"),
            )
            .field("aws_endpoint_url", &self.aws_endpoint_url)
            .field("ses_configuration_set", &self.ses_configuration_set)
            .field("aws_session_name", &self.aws_session_name)
            .field("aws_external_id", &self.aws_external_id)
            .finish()
    }
}

impl EmailConfig {
    /// Create a new SMTP-based `EmailConfig` with the given host and sender.
    ///
    /// This constructor preserves backward compatibility with the original API.
    ///
    /// # Examples
    ///
    /// ```
    /// use acteon_email::EmailConfig;
    ///
    /// let config = EmailConfig::new("smtp.example.com", "noreply@example.com");
    /// assert_eq!(config.smtp_port, 587);
    /// assert!(config.tls);
    /// ```
    pub fn new(smtp_host: impl Into<String>, from_address: impl Into<String>) -> Self {
        Self {
            from_address: from_address.into(),
            backend: "smtp".to_owned(),
            smtp_host: smtp_host.into(),
            smtp_port: 587,
            username: None,
            password: None,
            tls: true,
            aws_region: None,
            aws_role_arn: None,
            aws_endpoint_url: None,
            ses_configuration_set: None,
            aws_session_name: None,
            aws_external_id: None,
        }
    }

    /// Create a new SES-based `EmailConfig`.
    pub fn ses(region: impl Into<String>, from_address: impl Into<String>) -> Self {
        Self {
            from_address: from_address.into(),
            backend: "ses".to_owned(),
            smtp_host: String::new(),
            smtp_port: 587,
            username: None,
            password: None,
            tls: true,
            aws_region: Some(region.into()),
            aws_role_arn: None,
            aws_endpoint_url: None,
            ses_configuration_set: None,
            aws_session_name: None,
            aws_external_id: None,
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

    /// Set whether TLS should be used for SMTP.
    #[must_use]
    pub fn with_tls(mut self, tls: bool) -> Self {
        self.tls = tls;
        self
    }

    /// Set the SES configuration set name.
    #[must_use]
    pub fn with_ses_configuration_set(mut self, name: impl Into<String>) -> Self {
        self.ses_configuration_set = Some(name.into());
        self
    }

    /// Set the AWS endpoint URL override.
    #[must_use]
    pub fn with_aws_endpoint_url(mut self, url: impl Into<String>) -> Self {
        self.aws_endpoint_url = Some(url.into());
        self
    }

    /// Set the AWS role ARN for cross-account SES access.
    #[must_use]
    pub fn with_aws_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.aws_role_arn = Some(role_arn.into());
        self
    }

    /// Set the STS session name for SES assume-role.
    #[must_use]
    pub fn with_aws_session_name(mut self, session_name: impl Into<String>) -> Self {
        self.aws_session_name = Some(session_name.into());
        self
    }

    /// Set the external ID for SES cross-account trust policies.
    #[must_use]
    pub fn with_aws_external_id(mut self, external_id: impl Into<String>) -> Self {
        self.aws_external_id = Some(external_id.into());
        self
    }

    /// Returns `true` if this config uses the SMTP backend.
    pub fn is_smtp(&self) -> bool {
        self.backend == "smtp"
    }

    /// Returns `true` if this config uses the SES backend.
    pub fn is_ses(&self) -> bool {
        self.backend == "ses"
    }

    /// Extract the SMTP-specific config.
    pub fn smtp_config(&self) -> SmtpConfig {
        SmtpConfig {
            smtp_host: self.smtp_host.clone(),
            smtp_port: self.smtp_port,
            username: self.username.clone(),
            password: self.password.clone(),
            tls: self.tls,
        }
    }

    /// Extract the SES-specific config (requires the `ses` feature).
    #[cfg(feature = "ses")]
    pub fn ses_config(&self) -> acteon_aws::ses::SesConfig {
        let mut config =
            acteon_aws::ses::SesConfig::new(self.aws_region.as_deref().unwrap_or("us-east-1"));
        if let Some(ref set) = self.ses_configuration_set {
            config = config.with_configuration_set(set);
        }
        if let Some(ref url) = self.aws_endpoint_url {
            config = config.with_endpoint_url(url);
        }
        if let Some(ref arn) = self.aws_role_arn {
            config = config.with_role_arn(arn);
        }
        if let Some(ref name) = self.aws_session_name {
            config = config.with_session_name(name);
        }
        if let Some(ref ext_id) = self.aws_external_id {
            config = config.with_external_id(ext_id);
        }
        config
    }

    // Backward-compat accessors used by existing code.

    /// Get the SMTP host (for backward compatibility).
    pub fn smtp_host(&self) -> Option<&str> {
        if self.smtp_host.is_empty() {
            None
        } else {
            Some(&self.smtp_host)
        }
    }

    /// Get TLS setting.
    pub fn tls(&self) -> bool {
        self.tls
    }
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            from_address: "noreply@localhost".to_owned(),
            backend: "smtp".to_owned(),
            smtp_host: "localhost".to_owned(),
            smtp_port: 587,
            username: None,
            password: None,
            tls: true,
            aws_region: None,
            aws_role_arn: None,
            aws_endpoint_url: None,
            ses_configuration_set: None,
            aws_session_name: None,
            aws_external_id: None,
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
        assert!(config.is_smtp());
    }

    #[test]
    fn new_config_sets_host_and_from() {
        let config = EmailConfig::new("smtp.gmail.com", "me@gmail.com");
        assert_eq!(config.smtp_host, "smtp.gmail.com");
        assert_eq!(config.from_address, "me@gmail.com");
        assert_eq!(config.smtp_port, 587);
        assert!(config.tls);
        assert!(config.is_smtp());
    }

    #[test]
    fn ses_config_constructor() {
        let config = EmailConfig::ses("us-east-1", "noreply@example.com");
        assert!(config.is_ses());
        assert!(!config.is_smtp());
        assert_eq!(config.aws_region.as_deref(), Some("us-east-1"));
        assert_eq!(config.from_address, "noreply@example.com");
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
    fn smtp_config_extraction() {
        let config = EmailConfig::new("smtp.example.com", "sender@example.com")
            .with_credentials("user", "pass")
            .with_port(465)
            .with_tls(false);

        let smtp = config.smtp_config();
        assert_eq!(smtp.smtp_host, "smtp.example.com");
        assert_eq!(smtp.smtp_port, 465);
        assert_eq!(smtp.username.as_deref(), Some("user"));
        assert!(!smtp.tls);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = EmailConfig::new("smtp.example.com", "test@example.com")
            .with_credentials("user", "myvalue")
            .with_port(465)
            .with_tls(false);

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EmailConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.smtp_host, "smtp.example.com");
        assert_eq!(deserialized.smtp_port, 465);
        assert_eq!(deserialized.username.as_deref(), Some("user"));
        assert_eq!(deserialized.password.as_deref(), Some("myvalue"));
        assert_eq!(deserialized.from_address, "test@example.com");
        assert!(!deserialized.tls);
    }

    #[test]
    fn debug_redacts_password() {
        let config = EmailConfig::new("smtp.example.com", "test@example.com")
            .with_credentials("user", "test-pw-placeholder");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"), "password must be redacted");
        assert!(
            !debug.contains("test-pw-placeholder"),
            "password must not appear in debug output"
        );
        assert!(
            debug.contains("smtp.example.com"),
            "non-secret fields should be visible"
        );
    }

    #[test]
    fn ses_config_debug_redacts_role_arn() {
        let config = EmailConfig::ses("us-east-1", "noreply@example.com")
            .with_aws_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("123:role/test"));
    }

    #[test]
    fn smtp_host_accessor() {
        let config = EmailConfig::new("smtp.example.com", "test@example.com");
        assert_eq!(config.smtp_host(), Some("smtp.example.com"));

        let ses_config = EmailConfig::ses("us-east-1", "test@example.com");
        assert!(ses_config.smtp_host().is_none());
    }
}
