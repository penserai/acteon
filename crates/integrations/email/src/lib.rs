pub mod backend;
pub mod config;
pub mod provider;
pub mod smtp;
pub mod types;

#[cfg(feature = "ses")]
pub mod ses;

pub use config::{EmailConfig, SmtpConfig};
pub use provider::EmailProvider;
pub use types::EmailPayload;

// Re-export backend trait for external use.
pub use backend::{EmailBackend, EmailMessage, EmailResult};
