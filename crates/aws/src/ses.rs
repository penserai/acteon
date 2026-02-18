use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS SES email backend.
///
/// Used by the email provider's SES backend to send emails via the
/// `SESv2` `SendEmail` API.
#[derive(Clone, Serialize, Deserialize)]
pub struct SesConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Optional SES configuration set name for tracking.
    pub configuration_set: Option<String>,
}

impl std::fmt::Debug for SesConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SesConfig")
            .field("aws", &self.aws)
            .field("configuration_set", &self.configuration_set)
            .finish()
    }
}

impl SesConfig {
    /// Create a new `SesConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            configuration_set: None,
        }
    }

    /// Set the SES configuration set name.
    #[must_use]
    pub fn with_configuration_set(mut self, name: impl Into<String>) -> Self {
        self.configuration_set = Some(name.into());
        self
    }

    /// Set the endpoint URL override (for `LocalStack`).
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.aws.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the IAM role ARN to assume.
    #[must_use]
    pub fn with_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.aws.role_arn = Some(role_arn.into());
        self
    }
}

/// AWS `SESv2` client wrapper for sending emails.
///
/// This struct is used by the email provider's SES backend. It is not a
/// standalone provider -- the unified `EmailProvider` delegates to it.
pub struct SesClient {
    config: SesConfig,
    client: aws_sdk_sesv2::Client,
}

impl std::fmt::Debug for SesClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SesClient")
            .field("config", &self.config)
            .field("client", &"<SesV2Client>")
            .finish()
    }
}

impl SesClient {
    /// Create a new `SesClient` by building an AWS SDK client.
    pub async fn new(config: SesConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_sesv2::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create a `SesClient` with a pre-built client (for testing).
    pub fn with_client(config: SesConfig, client: aws_sdk_sesv2::Client) -> Self {
        Self { config, client }
    }

    /// Send an email via SES.
    ///
    /// # Arguments
    ///
    /// * `from` - Sender email address
    /// * `to` - Recipient email address
    /// * `subject` - Email subject
    /// * `body_text` - Optional plain-text body
    /// * `body_html` - Optional HTML body
    /// * `cc` - Optional CC address
    /// * `bcc` - Optional BCC address
    /// * `reply_to` - Optional reply-to address
    #[allow(clippy::too_many_arguments)]
    pub async fn send_email(
        &self,
        from: &str,
        to: &str,
        subject: &str,
        body_text: Option<&str>,
        body_html: Option<&str>,
        cc: Option<&str>,
        bcc: Option<&str>,
        reply_to: Option<&str>,
    ) -> Result<String, acteon_provider::ProviderError> {
        debug!(from = %from, to = %to, subject = %subject, "sending email via SES");

        let mut destination = aws_sdk_sesv2::types::Destination::builder().to_addresses(to);
        if let Some(cc_addr) = cc {
            destination = destination.cc_addresses(cc_addr);
        }
        if let Some(bcc_addr) = bcc {
            destination = destination.bcc_addresses(bcc_addr);
        }

        let subject_content = aws_sdk_sesv2::types::Content::builder()
            .data(subject)
            .charset("UTF-8")
            .build()
            .map_err(|e| acteon_provider::ProviderError::Serialization(e.to_string()))?;

        let mut body_builder = aws_sdk_sesv2::types::Body::builder();
        if let Some(text) = body_text {
            body_builder = body_builder.text(
                aws_sdk_sesv2::types::Content::builder()
                    .data(text)
                    .charset("UTF-8")
                    .build()
                    .map_err(|e| acteon_provider::ProviderError::Serialization(e.to_string()))?,
            );
        }
        if let Some(html) = body_html {
            body_builder = body_builder.html(
                aws_sdk_sesv2::types::Content::builder()
                    .data(html)
                    .charset("UTF-8")
                    .build()
                    .map_err(|e| acteon_provider::ProviderError::Serialization(e.to_string()))?,
            );
        }

        let message = aws_sdk_sesv2::types::Message::builder()
            .subject(subject_content)
            .body(body_builder.build())
            .build();

        let email_content = aws_sdk_sesv2::types::EmailContent::builder()
            .simple(message)
            .build();

        let mut request = self
            .client
            .send_email()
            .from_email_address(from)
            .destination(destination.build())
            .content(email_content);

        if let Some(reply) = reply_to {
            request = request.reply_to_addresses(reply);
        }

        if let Some(ref config_set) = self.config.configuration_set {
            request = request.configuration_set_name(config_set);
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "SES send_email failed");
            let aws_err: acteon_provider::ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let message_id = result.message_id().unwrap_or("unknown").to_owned();
        info!(message_id = %message_id, "SES email sent");

        Ok(message_id)
    }

    /// Perform a health check by calling the SES `GetAccount` API.
    pub async fn health_check(&self) -> Result<(), acteon_provider::ProviderError> {
        debug!("performing SES health check");
        self.client.get_account().send().await.map_err(|e| {
            error!(error = %e, "SES health check failed");
            acteon_provider::ProviderError::Connection(format!("SES health check failed: {e}"))
        })?;
        info!("SES health check passed");
        Ok(())
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &SesConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = SesConfig::new("us-east-1");
        assert_eq!(config.aws.region, "us-east-1");
        assert!(config.configuration_set.is_none());
    }

    #[test]
    fn config_with_configuration_set() {
        let config = SesConfig::new("us-east-1").with_configuration_set("tracking-set");
        assert_eq!(config.configuration_set.as_deref(), Some("tracking-set"));
    }

    #[test]
    fn config_builder_chain() {
        let config = SesConfig::new("eu-west-1")
            .with_configuration_set("my-set")
            .with_endpoint_url("http://localhost:4566")
            .with_role_arn("arn:aws:iam::123:role/ses");
        assert_eq!(config.aws.region, "eu-west-1");
        assert_eq!(config.configuration_set.as_deref(), Some("my-set"));
        assert!(config.aws.endpoint_url.is_some());
        assert!(config.aws.role_arn.is_some());
    }

    #[test]
    fn config_debug_format() {
        let config = SesConfig::new("us-east-1").with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("SesConfig"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = SesConfig::new("us-west-2").with_configuration_set("test-set");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SesConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "us-west-2");
        assert_eq!(deserialized.configuration_set.as_deref(), Some("test-set"));
    }
}
