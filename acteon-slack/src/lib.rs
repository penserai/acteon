//! Slack provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to deliver messages through the
//! [Slack Web API](https://api.slack.com/web).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_slack::{SlackConfig, SlackProvider};
//!
//! let config = SlackConfig::new("xoxb-your-bot-token")
//!     .with_default_channel("#alerts");
//! let provider = SlackProvider::new(config);
//! ```

pub mod config;
pub mod error;
pub mod image_gen;
pub mod provider;
pub mod types;

pub use config::SlackConfig;
pub use error::SlackError;
pub use image_gen::ImageSpec;
pub use provider::SlackProvider;
pub use types::{
    SlackApiResponse, SlackAuthTestResponse, SlackCompleteUploadRequest,
    SlackCompleteUploadResponse, SlackFileReference, SlackGetUploadUrlRequest,
    SlackGetUploadUrlResponse, SlackPostMessageRequest,
};
