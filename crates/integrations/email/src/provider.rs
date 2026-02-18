use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::provider::Provider;
use tracing::{debug, info, instrument};

use crate::backend::{EmailBackend, EmailMessage};
use crate::config::EmailConfig;
use crate::smtp::SmtpBackend;
use crate::types::EmailPayload;

/// An email provider that sends messages through a pluggable backend.
///
/// Supports SMTP (default) and SES (with the `ses` feature) backends.
/// Implements the [`Provider`] trait from `acteon-provider`, mapping action
/// payloads to email messages and dispatching them through the configured
/// backend.
///
/// # Examples
///
/// ```no_run
/// use acteon_email::{EmailConfig, EmailProvider};
///
/// let config = EmailConfig::new("smtp.example.com", "noreply@example.com")
///     .with_credentials("user", "pass");
/// let provider = EmailProvider::new(&config).unwrap();
/// assert_eq!(acteon_provider::Provider::name(&provider), "email");
/// ```
pub struct EmailProvider {
    from_address: String,
    backend: Box<dyn EmailBackend>,
}

impl std::fmt::Debug for EmailProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailProvider")
            .field("from_address", &self.from_address)
            .field("backend", &self.backend)
            .finish()
    }
}

impl EmailProvider {
    /// Create a new `EmailProvider` from the given configuration.
    ///
    /// Automatically selects the SMTP backend. For SES, use
    /// [`EmailProvider::ses`] instead.
    ///
    /// Returns a [`ProviderError::Configuration`] if the SMTP transport
    /// cannot be built.
    pub fn new(config: &EmailConfig) -> Result<Self, ProviderError> {
        let from_address = config.from_address.clone();
        let smtp_config = config.smtp_config();
        let backend = SmtpBackend::new(smtp_config)?;
        Ok(Self {
            from_address,
            backend: Box::new(backend),
        })
    }

    /// Create a new `EmailProvider` with an SMTP backend.
    pub fn smtp(config: &EmailConfig) -> Result<Self, ProviderError> {
        Self::new(config)
    }

    /// Create a new `EmailProvider` with an SES backend.
    #[cfg(feature = "ses")]
    pub async fn ses(config: &EmailConfig) -> Self {
        let from_address = config.from_address.clone();
        let ses_config = config.ses_config();
        let backend = crate::ses::SesBackend::new(ses_config).await;
        Self {
            from_address,
            backend: Box::new(backend),
        }
    }

    /// Create an `EmailProvider` with a pre-built backend (for testing).
    pub fn with_backend(from_address: impl Into<String>, backend: Box<dyn EmailBackend>) -> Self {
        Self {
            from_address: from_address.into(),
            backend,
        }
    }

    /// Create an `EmailProvider` with a pre-built SMTP transport (for testing).
    ///
    /// Preserves the original API for test backward compatibility.
    pub fn with_transport(
        config: &EmailConfig,
        transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    ) -> Self {
        let from_address = config.from_address.clone();
        let smtp_config = config.smtp_config();
        let backend = SmtpBackend::with_transport(smtp_config, transport);
        Self {
            from_address,
            backend: Box::new(backend),
        }
    }
}

impl Provider for EmailProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "email"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "email"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing email payload");
        let payload: EmailPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let message = EmailMessage {
            from: self.from_address.clone(),
            to: payload.to.clone(),
            subject: payload.subject.clone(),
            body: payload.body.clone(),
            html_body: payload.html_body.clone(),
            cc: payload.cc.clone(),
            bcc: payload.bcc.clone(),
            reply_to: payload.reply_to.clone(),
        };

        debug!(
            to = %message.to,
            subject = %message.subject,
            backend = self.backend.backend_name(),
            "sending email"
        );

        let result = self.backend.send(&message).await?;

        info!(
            to = %payload.to,
            backend = self.backend.backend_name(),
            "email sent successfully"
        );

        let mut response = serde_json::json!({
            "to": payload.to,
            "subject": payload.subject,
            "status": result.status,
            "backend": self.backend.backend_name()
        });

        if let Some(ref msg_id) = result.message_id {
            response["message_id"] = serde_json::Value::String(msg_id.clone());
        }

        Ok(ProviderResponse::success(response))
    }

    #[instrument(skip(self), fields(provider = "email"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        self.backend.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;
    use lettre::{AsyncSmtpTransport, Tokio1Executor};

    use super::*;

    /// Helper to create an action with the given JSON payload.
    fn make_action(payload: serde_json::Value) -> Action {
        Action::new("notifications", "tenant-1", "email", "send_email", payload)
    }

    /// Helper to build a test config (no TLS, pointed at localhost).
    fn test_config() -> EmailConfig {
        EmailConfig::new("localhost", "sender@example.com").with_tls(false)
    }

    // -----------------------------------------------------------------------
    // Provider / transport tests (require tokio runtime)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn provider_name_is_email() {
        let config = test_config();
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(&config, transport);
        assert_eq!(Provider::name(&provider), "email");
    }

    #[tokio::test]
    async fn execute_invalid_payload_returns_serialization_error() {
        let config = EmailConfig::default().with_tls(false);
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(&config, transport);

        // Missing required fields "to" and "subject"
        let action = make_action(serde_json::json!({"invalid": "data"}));
        let result = Provider::execute(&provider, &action).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }

    #[tokio::test]
    async fn execute_invalid_recipient_returns_execution_error() {
        let config = test_config();
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(&config, transport);

        let action = make_action(serde_json::json!({
            "to": "not-an-email",
            "subject": "Test"
        }));
        let result = Provider::execute(&provider, &action).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn new_with_tls_and_empty_host_builds_transport() {
        let config = EmailConfig::new("", "sender@example.com");
        let result = EmailProvider::new(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn new_without_tls_succeeds() {
        let config = EmailConfig::new("localhost", "sender@example.com").with_tls(false);
        let result = EmailProvider::new(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn new_with_credentials() {
        let config = EmailConfig::new("localhost", "sender@example.com")
            .with_tls(false)
            .with_credentials("user", "pass");
        let result = EmailProvider::new(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn debug_impl_does_not_panic() {
        let config = test_config();
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(&config, transport);
        let debug_str = format!("{provider:?}");
        assert!(debug_str.contains("EmailProvider"));
        assert!(debug_str.contains("SmtpBackend"));
    }

    #[tokio::test]
    async fn smtp_constructor_alias() {
        let config = test_config();
        let provider = EmailProvider::smtp(&config);
        assert!(provider.is_ok());
        assert_eq!(Provider::name(&provider.unwrap()), "email");
    }
}
