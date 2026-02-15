//! Discord provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to deliver messages through
//! [Discord webhooks](https://discord.com/developers/docs/resources/webhook).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_discord::{DiscordConfig, DiscordProvider};
//!
//! let config = DiscordConfig::new("https://discord.com/api/webhooks/123/abc")
//!     .with_default_username("Acteon Bot");
//! let provider = DiscordProvider::new(config);
//! ```

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::DiscordConfig;
pub use error::DiscordError;
pub use provider::DiscordProvider;
pub use types::{DiscordEmbed, DiscordWebhookRequest};
