use acteon_core::{Action, ProviderResponse};
use acteon_provider::provider::Provider;
use acteon_provider::ProviderError;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tracing::{debug, error, info, instrument};

use crate::config::EmailConfig;
use crate::types::EmailPayload;

/// An email provider that sends messages via SMTP using `lettre`.
///
/// Implements the [`Provider`] trait from `acteon-provider`, mapping action
/// payloads to email messages and dispatching them through a configured SMTP
/// transport.
///
/// # Examples
///
/// ```no_run
/// use acteon_email::{EmailConfig, EmailProvider};
///
/// let config = EmailConfig::new("smtp.example.com", "noreply@example.com")
///     .with_credentials("user", "pass");
/// let provider = EmailProvider::new(config).unwrap();
/// assert_eq!(acteon_provider::Provider::name(&provider), "email");
/// ```
pub struct EmailProvider {
    config: EmailConfig,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl std::fmt::Debug for EmailProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailProvider")
            .field("config", &self.config)
            .field("transport", &"<AsyncSmtpTransport>")
            .finish()
    }
}

impl EmailProvider {
    /// Create a new `EmailProvider` from the given configuration.
    ///
    /// Builds an [`AsyncSmtpTransport`] configured with the SMTP host, port,
    /// TLS settings, and optional credentials from the [`EmailConfig`].
    ///
    /// Returns a [`ProviderError::Configuration`] if the transport cannot be
    /// built (e.g. invalid host).
    pub fn new(config: EmailConfig) -> Result<Self, ProviderError> {
        let transport = build_transport(&config)?;
        Ok(Self { config, transport })
    }

    /// Create an `EmailProvider` with a pre-built transport.
    ///
    /// This is primarily useful for testing, allowing injection of a custom
    /// or mock transport.
    pub fn with_transport(
        config: EmailConfig,
        transport: AsyncSmtpTransport<Tokio1Executor>,
    ) -> Self {
        Self { config, transport }
    }
}

/// Build a `lettre::Message` from the email payload and sender config.
///
/// This is a free function so it can be tested independently of the async
/// SMTP transport (which requires a Tokio runtime to construct).
fn build_message(config: &EmailConfig, payload: &EmailPayload) -> Result<Message, ProviderError> {
    let from_mailbox: Mailbox = config
        .from_address
        .parse()
        .map_err(|e| ProviderError::Configuration(format!("invalid from address: {e}")))?;

    let to_mailbox: Mailbox = payload
        .to
        .parse()
        .map_err(|e| ProviderError::ExecutionFailed(format!("invalid recipient address: {e}")))?;

    let mut builder = Message::builder()
        .from(from_mailbox)
        .to(to_mailbox)
        .subject(&payload.subject);

    if let Some(ref reply_to) = payload.reply_to {
        let reply_mailbox: Mailbox = reply_to.parse().map_err(|e| {
            ProviderError::ExecutionFailed(format!("invalid reply-to address: {e}"))
        })?;
        builder = builder.reply_to(reply_mailbox);
    }

    if let Some(ref cc) = payload.cc {
        let cc_mailbox: Mailbox = cc
            .parse()
            .map_err(|e| ProviderError::ExecutionFailed(format!("invalid CC address: {e}")))?;
        builder = builder.cc(cc_mailbox);
    }

    if let Some(ref bcc) = payload.bcc {
        let bcc_mailbox: Mailbox = bcc
            .parse()
            .map_err(|e| ProviderError::ExecutionFailed(format!("invalid BCC address: {e}")))?;
        builder = builder.bcc(bcc_mailbox);
    }

    let message = match (&payload.body, &payload.html_body) {
        (Some(text), Some(html)) => builder
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_PLAIN)
                            .body(text.clone()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_HTML)
                            .body(html.clone()),
                    ),
            )
            .map_err(|e| ProviderError::ExecutionFailed(format!("failed to build email: {e}")))?,
        (Some(text), None) => builder
            .body(text.clone())
            .map_err(|e| ProviderError::ExecutionFailed(format!("failed to build email: {e}")))?,
        (None, Some(html)) => builder
            .singlepart(
                SinglePart::builder()
                    .header(lettre::message::header::ContentType::TEXT_HTML)
                    .body(html.clone()),
            )
            .map_err(|e| ProviderError::ExecutionFailed(format!("failed to build email: {e}")))?,
        (None, None) => builder
            .body(String::new())
            .map_err(|e| ProviderError::ExecutionFailed(format!("failed to build email: {e}")))?,
    };

    Ok(message)
}

/// Build an async SMTP transport from the given configuration.
fn build_transport(
    config: &EmailConfig,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, ProviderError> {
    let builder = if config.tls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
            .map_err(|e| ProviderError::Configuration(format!("SMTP TLS relay error: {e}")))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
    };

    let builder = builder.port(config.smtp_port);

    let builder = if let (Some(ref user), Some(ref pass)) = (&config.username, &config.password) {
        builder.credentials(Credentials::new(user.clone(), pass.clone()))
    } else {
        builder
    };

    Ok(builder.build())
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

        debug!(to = %payload.to, subject = %payload.subject, "building email message");
        let message = build_message(&self.config, &payload)?;

        info!(to = %payload.to, subject = %payload.subject, "sending email");
        self.transport.send(message).await.map_err(|e| {
            error!(error = %e, "SMTP send failed");
            map_smtp_error(&e)
        })?;

        info!(to = %payload.to, "email sent successfully");
        Ok(ProviderResponse::success(serde_json::json!({
            "to": payload.to,
            "subject": payload.subject,
            "status": "sent"
        })))
    }

    #[instrument(skip(self), fields(provider = "email"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing SMTP health check");
        self.transport.test_connection().await.map_err(|e| {
            error!(error = %e, "SMTP health check failed");
            ProviderError::Connection(format!("SMTP health check failed: {e}"))
        })?;
        info!("SMTP health check passed");
        Ok(())
    }
}

/// Map a lettre SMTP error to the appropriate `ProviderError` variant.
fn map_smtp_error(error: &lettre::transport::smtp::Error) -> ProviderError {
    let message = error.to_string();

    if error.is_transient() {
        ProviderError::Connection(format!("transient SMTP error: {message}"))
    } else if error.is_permanent() {
        ProviderError::ExecutionFailed(format!("permanent SMTP error: {message}"))
    } else {
        // Covers TLS, connection, response parsing, and other errors.
        ProviderError::Connection(format!("SMTP error: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use acteon_core::Action;

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
    // Message building tests (synchronous -- no transport needed)
    // -----------------------------------------------------------------------

    #[test]
    fn build_message_plain_text() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Test Subject".to_owned(),
            body: Some("Hello, world!".to_owned()),
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let message = build_message(&config, &payload);
        assert!(message.is_ok());
    }

    #[test]
    fn build_message_html_only() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "HTML Test".to_owned(),
            body: None,
            html_body: Some("<h1>Hello</h1>".to_owned()),
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let message = build_message(&config, &payload);
        assert!(message.is_ok());
    }

    #[test]
    fn build_message_multipart() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Multipart".to_owned(),
            body: Some("Plain text".to_owned()),
            html_body: Some("<p>HTML text</p>".to_owned()),
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let message = build_message(&config, &payload);
        assert!(message.is_ok());
    }

    #[test]
    fn build_message_with_all_recipients() {
        let config = test_config();
        let payload = EmailPayload {
            to: "to@example.com".to_owned(),
            subject: "Full".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: Some("cc@example.com".to_owned()),
            bcc: Some("bcc@example.com".to_owned()),
            reply_to: Some("reply@example.com".to_owned()),
        };

        let message = build_message(&config, &payload);
        assert!(message.is_ok());
    }

    #[test]
    fn build_message_invalid_from_address() {
        let mut config = test_config();
        config.from_address = "not-valid".to_owned();

        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let result = build_message(&config, &payload);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn build_message_invalid_to_address() {
        let config = test_config();
        let payload = EmailPayload {
            to: "not-valid".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let result = build_message(&config, &payload);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn build_message_empty_body() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Empty".to_owned(),
            body: None,
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: None,
        };

        let message = build_message(&config, &payload);
        assert!(message.is_ok());
    }

    #[test]
    fn build_message_invalid_cc_address() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: Some("bad-cc".to_owned()),
            bcc: None,
            reply_to: None,
        };

        let result = build_message(&config, &payload);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn build_message_invalid_bcc_address() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: None,
            bcc: Some("bad-bcc".to_owned()),
            reply_to: None,
        };

        let result = build_message(&config, &payload);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn build_message_invalid_reply_to_address() {
        let config = test_config();
        let payload = EmailPayload {
            to: "recipient@example.com".to_owned(),
            subject: "Test".to_owned(),
            body: Some("body".to_owned()),
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: Some("bad-reply".to_owned()),
        };

        let result = build_message(&config, &payload);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
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
        let provider = EmailProvider::with_transport(config, transport);
        assert_eq!(Provider::name(&provider), "email");
    }

    #[tokio::test]
    async fn execute_invalid_payload_returns_serialization_error() {
        let config = EmailConfig::default().with_tls(false);
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(config, transport);

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
        let provider = EmailProvider::with_transport(config, transport);

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
        // lettre accepts empty hostnames at build time; the error surfaces
        // at connection time. Verify the provider can still be constructed.
        let config = EmailConfig::new("", "sender@example.com");
        let result = EmailProvider::new(config);
        // This currently succeeds because lettre defers DNS resolution.
        // The connection error will surface during execute/health_check.
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn build_transport_no_tls_no_credentials() {
        // Verify a basic transport can be built without TLS or credentials.
        let config = EmailConfig {
            smtp_host: "localhost".to_owned(),
            smtp_port: 2525,
            username: None,
            password: None,
            from_address: "sender@example.com".to_owned(),
            tls: false,
        };
        let result = build_transport(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn build_transport_no_tls_with_credentials() {
        // Verify a transport can be built with credentials but without TLS.
        let config = EmailConfig {
            smtp_host: "localhost".to_owned(),
            smtp_port: 2525,
            username: Some("user".to_owned()),
            password: Some("pass".to_owned()),
            from_address: "sender@example.com".to_owned(),
            tls: false,
        };
        let result = build_transport(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn new_without_tls_succeeds() {
        let config = EmailConfig::new("localhost", "sender@example.com").with_tls(false);
        let result = EmailProvider::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn new_with_credentials() {
        let config = EmailConfig::new("localhost", "sender@example.com")
            .with_tls(false)
            .with_credentials("user", "pass");
        let result = EmailProvider::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn debug_impl_does_not_panic() {
        let config = test_config();
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let provider = EmailProvider::with_transport(config, transport);
        let debug_str = format!("{provider:?}");
        assert!(debug_str.contains("EmailProvider"));
        assert!(debug_str.contains("AsyncSmtpTransport"));
    }
}
