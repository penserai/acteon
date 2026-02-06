//! `PagerDuty` provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to send events through the
//! [PagerDuty Events API v2](https://developer.pagerduty.com/docs/events-api-v2/overview/).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_pagerduty::{PagerDutyConfig, PagerDutyProvider};
//!
//! // Single service
//! let config = PagerDutyConfig::single_service("PABC123", "your-routing-key")
//!     .with_default_severity("critical")
//!     .with_default_source("monitoring");
//! let provider = PagerDutyProvider::new(config);
//!
//! // Multiple services
//! let config = PagerDutyConfig::new()
//!     .with_service("PABC123", "routing-key-1")
//!     .with_service("PXYZ789", "routing-key-2")
//!     .with_default_service("PABC123");
//! let provider = PagerDutyProvider::new(config);
//! ```

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::PagerDutyConfig;
pub use error::PagerDutyError;
pub use provider::PagerDutyProvider;
pub use types::{
    PagerDutyApiResponse, PagerDutyEvent, PagerDutyImage, PagerDutyLink, PagerDutyPayload,
};
