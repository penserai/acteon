//! Microsoft Teams provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to deliver messages through
//! [Microsoft Teams incoming webhooks](https://learn.microsoft.com/en-us/microsoftteams/platform/webhooks-and-connectors/how-to/add-incoming-webhook).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_teams::{TeamsConfig, TeamsProvider};
//!
//! let config = TeamsConfig::new("https://outlook.office.com/webhook/...");
//! let provider = TeamsProvider::new(config);
//! ```

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::TeamsConfig;
pub use error::TeamsError;
pub use provider::TeamsProvider;
pub use types::TeamsMessageCard;
