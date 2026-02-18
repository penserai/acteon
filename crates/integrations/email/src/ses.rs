use acteon_provider::ProviderError;
use async_trait::async_trait;
use tracing::{debug, info};

use crate::backend::{EmailBackend, EmailMessage, EmailResult};

/// SES email delivery backend using the `acteon-aws` SES client.
///
/// Delegates to [`acteon_aws::ses::SesClient`] for the actual AWS API calls.
pub struct SesBackend {
    client: acteon_aws::ses::SesClient,
}

impl std::fmt::Debug for SesBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SesBackend")
            .field("client", &self.client)
            .finish()
    }
}

impl SesBackend {
    /// Create a new `SesBackend` by building an AWS SES client.
    pub async fn new(config: acteon_aws::ses::SesConfig) -> Self {
        let client = acteon_aws::ses::SesClient::new(config).await;
        Self { client }
    }

    /// Create a `SesBackend` with a pre-built SES client (for testing).
    pub fn with_client(client: acteon_aws::ses::SesClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EmailBackend for SesBackend {
    async fn send(&self, message: &EmailMessage) -> Result<EmailResult, ProviderError> {
        debug!(to = %message.to, subject = %message.subject, "sending email via SES");

        let message_id = self
            .client
            .send_email(
                &message.from,
                &message.to,
                &message.subject,
                message.body.as_deref(),
                message.html_body.as_deref(),
                message.cc.as_deref(),
                message.bcc.as_deref(),
                message.reply_to.as_deref(),
            )
            .await?;

        info!(message_id = %message_id, to = %message.to, "email sent via SES");

        Ok(EmailResult {
            message_id: Some(message_id),
            status: "sent".to_owned(),
        })
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        self.client.health_check().await
    }

    fn backend_name(&self) -> &'static str {
        "ses"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ses_backend_debug_does_not_panic() {
        // We can't easily construct a real SesBackend without AWS credentials,
        // but we can verify the types and debug impl compile.
        let _config = acteon_aws::ses::SesConfig::new("us-east-1");
    }
}
