//! Generic HTTP webhook provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to deliver actions to any HTTP endpoint with configurable
//! methods, authentication, headers, payload transforms, and response
//! validation.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_webhook::{WebhookConfig, WebhookProvider};
//!
//! // Simple POST webhook
//! let config = WebhookConfig::new("https://api.example.com/webhook");
//! let provider = WebhookProvider::new("my-webhook", config);
//!
//! // With authentication and custom headers
//! use acteon_webhook::{AuthMethod, HttpMethod};
//! let config = WebhookConfig::new("https://api.example.com/events")
//!     .with_method(HttpMethod::Put)
//!     .with_auth(AuthMethod::Bearer("token-123".into()))
//!     .with_header("X-Custom", "value")
//!     .with_timeout_secs(15);
//! let provider = WebhookProvider::new("custom-hook", config);
//! ```

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::{AuthMethod, HttpMethod, PayloadMode, WebhookConfig};
pub use error::WebhookError;
pub use provider::WebhookProvider;
pub use types::WebhookResponse;
