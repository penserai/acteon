use acteon_provider::ProviderError;
use async_trait::async_trait;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tracing::{debug, error, info};

use crate::backend::{EmailBackend, EmailMessage, EmailResult};
use crate::config::SmtpConfig;

/// SMTP email delivery backend using `lettre`.
pub struct SmtpBackend {
    config: SmtpConfig,
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl std::fmt::Debug for SmtpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpBackend")
            .field("config", &self.config)
            .field("transport", &"<AsyncSmtpTransport>")
            .finish()
    }
}

impl SmtpBackend {
    /// Create a new `SmtpBackend` from the given SMTP configuration.
    pub fn new(config: SmtpConfig) -> Result<Self, ProviderError> {
        let transport = build_transport(&config)?;
        Ok(Self { config, transport })
    }

    /// Create a `SmtpBackend` with a pre-built transport (for testing).
    pub fn with_transport(
        config: SmtpConfig,
        transport: AsyncSmtpTransport<Tokio1Executor>,
    ) -> Self {
        Self { config, transport }
    }
}

#[async_trait]
impl EmailBackend for SmtpBackend {
    async fn send(&self, message: &EmailMessage) -> Result<EmailResult, ProviderError> {
        debug!(to = %message.to, subject = %message.subject, "building SMTP message");
        let lettre_message = build_message(message)?;

        info!(to = %message.to, subject = %message.subject, "sending email via SMTP");
        self.transport.send(lettre_message).await.map_err(|e| {
            error!(error = %e, "SMTP send failed");
            map_smtp_error(&e)
        })?;

        info!(to = %message.to, "email sent successfully via SMTP");
        Ok(EmailResult {
            message_id: None,
            status: "sent".to_owned(),
        })
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing SMTP health check");
        self.transport.test_connection().await.map_err(|e| {
            error!(error = %e, "SMTP health check failed");
            ProviderError::Connection(format!("SMTP health check failed: {e}"))
        })?;
        info!("SMTP health check passed");
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "smtp"
    }
}

/// Build a `lettre::Message` from the unified [`EmailMessage`].
fn build_message(msg: &EmailMessage) -> Result<Message, ProviderError> {
    let from_mailbox: Mailbox = msg
        .from
        .parse()
        .map_err(|e| ProviderError::Configuration(format!("invalid from address: {e}")))?;

    let to_mailbox: Mailbox = msg
        .to
        .parse()
        .map_err(|e| ProviderError::ExecutionFailed(format!("invalid recipient address: {e}")))?;

    let mut builder = Message::builder()
        .from(from_mailbox)
        .to(to_mailbox)
        .subject(&msg.subject);

    if let Some(ref reply_to) = msg.reply_to {
        let reply_mailbox: Mailbox = reply_to.parse().map_err(|e| {
            ProviderError::ExecutionFailed(format!("invalid reply-to address: {e}"))
        })?;
        builder = builder.reply_to(reply_mailbox);
    }

    if let Some(ref cc) = msg.cc {
        let cc_mailbox: Mailbox = cc
            .parse()
            .map_err(|e| ProviderError::ExecutionFailed(format!("invalid CC address: {e}")))?;
        builder = builder.cc(cc_mailbox);
    }

    if let Some(ref bcc) = msg.bcc {
        let bcc_mailbox: Mailbox = bcc
            .parse()
            .map_err(|e| ProviderError::ExecutionFailed(format!("invalid BCC address: {e}")))?;
        builder = builder.bcc(bcc_mailbox);
    }

    let message = match (&msg.body, &msg.html_body) {
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
    config: &SmtpConfig,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, ProviderError> {
    let builder = if config.tls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp_host)
            .map_err(|e| ProviderError::Configuration(format!("SMTP TLS relay error: {e}")))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp_host)
    };

    let builder = builder.port(config.smtp_port);

    let builder = if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        builder.credentials(Credentials::new(user.clone(), pass.clone()))
    } else {
        builder
    };

    Ok(builder.build())
}

/// Map a lettre SMTP error to the appropriate `ProviderError` variant.
fn map_smtp_error(error: &lettre::transport::smtp::Error) -> ProviderError {
    let message = error.to_string();

    if error.is_transient() {
        ProviderError::Connection(format!("transient SMTP error: {message}"))
    } else if error.is_permanent() {
        ProviderError::ExecutionFailed(format!("permanent SMTP error: {message}"))
    } else {
        ProviderError::Connection(format!("SMTP error: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use lettre::{AsyncSmtpTransport, Tokio1Executor};

    use super::*;

    fn test_smtp_config() -> SmtpConfig {
        SmtpConfig {
            smtp_host: "localhost".to_owned(),
            smtp_port: 2525,
            username: None,
            password: None,
            tls: false,
        }
    }

    fn test_message() -> EmailMessage {
        EmailMessage {
            from: "sender@example.com".to_owned(),
            to: "recipient@example.com".to_owned(),
            subject: "Test Subject".to_owned(),
            body: Some("Hello, world!".to_owned()),
            html_body: None,
            cc: None,
            bcc: None,
            reply_to: None,
        }
    }

    #[test]
    fn build_message_plain_text() {
        let msg = test_message();
        assert!(build_message(&msg).is_ok());
    }

    #[test]
    fn build_message_html_only() {
        let mut msg = test_message();
        msg.body = None;
        msg.html_body = Some("<h1>Hello</h1>".to_owned());
        assert!(build_message(&msg).is_ok());
    }

    #[test]
    fn build_message_multipart() {
        let mut msg = test_message();
        msg.html_body = Some("<p>Hello</p>".to_owned());
        assert!(build_message(&msg).is_ok());
    }

    #[test]
    fn build_message_with_all_recipients() {
        let mut msg = test_message();
        msg.cc = Some("cc@example.com".to_owned());
        msg.bcc = Some("bcc@example.com".to_owned());
        msg.reply_to = Some("reply@example.com".to_owned());
        assert!(build_message(&msg).is_ok());
    }

    #[test]
    fn build_message_invalid_from() {
        let mut msg = test_message();
        msg.from = "not-valid".to_owned();
        let err = build_message(&msg).unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));
    }

    #[test]
    fn build_message_invalid_to() {
        let mut msg = test_message();
        msg.to = "not-valid".to_owned();
        let err = build_message(&msg).unwrap_err();
        assert!(matches!(err, ProviderError::ExecutionFailed(_)));
    }

    #[test]
    fn build_message_empty_body() {
        let mut msg = test_message();
        msg.body = None;
        msg.html_body = None;
        assert!(build_message(&msg).is_ok());
    }

    #[tokio::test]
    async fn build_transport_no_tls() {
        let config = test_smtp_config();
        assert!(build_transport(&config).is_ok());
    }

    #[tokio::test]
    async fn build_transport_with_credentials() {
        let mut config = test_smtp_config();
        config.username = Some("user".to_owned());
        config.password = Some("pass".to_owned());
        assert!(build_transport(&config).is_ok());
    }

    #[tokio::test]
    async fn smtp_backend_new() {
        let config = test_smtp_config();
        let backend = SmtpBackend::new(config);
        assert!(backend.is_ok());
    }

    #[tokio::test]
    async fn smtp_backend_name() {
        let config = test_smtp_config();
        let backend = SmtpBackend::new(config).unwrap();
        assert_eq!(backend.backend_name(), "smtp");
    }

    #[tokio::test]
    async fn smtp_backend_debug() {
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous("localhost")
            .port(2525)
            .build();
        let backend = SmtpBackend::with_transport(test_smtp_config(), transport);
        let debug = format!("{backend:?}");
        assert!(debug.contains("SmtpBackend"));
    }
}
