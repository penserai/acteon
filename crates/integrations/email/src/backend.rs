use acteon_provider::ProviderError;
use async_trait::async_trait;

/// A unified email message representation shared across all backends.
#[derive(Debug, Clone)]
pub struct EmailMessage {
    /// Sender email address.
    pub from: String,
    /// Recipient email address.
    pub to: String,
    /// Email subject line.
    pub subject: String,
    /// Optional plain-text body.
    pub body: Option<String>,
    /// Optional HTML body.
    pub html_body: Option<String>,
    /// Optional CC address.
    pub cc: Option<String>,
    /// Optional BCC address.
    pub bcc: Option<String>,
    /// Optional reply-to address.
    pub reply_to: Option<String>,
}

/// Result of a successful email send operation.
#[derive(Debug, Clone)]
pub struct EmailResult {
    /// Provider-assigned message identifier (if available).
    pub message_id: Option<String>,
    /// Human-readable status (e.g. `"sent"`, `"queued"`).
    pub status: String,
}

/// Trait for pluggable email delivery backends.
///
/// Implementations handle the actual transport of email messages (SMTP, SES,
/// etc.) while the [`EmailProvider`](crate::provider::EmailProvider) handles
/// payload deserialization and the `Provider` trait interface.
#[async_trait]
pub trait EmailBackend: Send + Sync + std::fmt::Debug {
    /// Send an email message through this backend.
    async fn send(&self, message: &EmailMessage) -> Result<EmailResult, ProviderError>;

    /// Perform a health check to verify the backend is operational.
    async fn health_check(&self) -> Result<(), ProviderError>;

    /// Return the backend name (e.g. `"smtp"`, `"ses"`).
    fn backend_name(&self) -> &'static str;
}
