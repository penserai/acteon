//! Twilio SMS provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to deliver SMS messages through the
//! [Twilio REST API](https://www.twilio.com/docs/sms/api/message-resource).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_twilio::{TwilioConfig, TwilioProvider};
//!
//! let config = TwilioConfig::new("ACXXXXXXXX", "auth_token")
//!     .with_from_number("+15551234567");
//! let provider = TwilioProvider::new(config);
//! ```

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::TwilioConfig;
pub use error::TwilioError;
pub use provider::TwilioProvider;
pub use types::{TwilioApiResponse, TwilioSendMessageRequest};
