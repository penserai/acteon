//! `OpsGenie` provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait,
//! enabling Acteon to send alerts through the
//! [OpsGenie Alert API v2](https://docs.opsgenie.com/docs/alert-api).
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_opsgenie::{OpsGenieConfig, OpsGenieProvider, OpsGenieRegion};
//!
//! // Single team, US region.
//! let config = OpsGenieConfig::new("your-api-key-here")
//!     .with_region(OpsGenieRegion::Us)
//!     .with_default_team("ops-team")
//!     .with_default_priority("P3")
//!     .with_default_source("acteon");
//! let provider = OpsGenieProvider::new(config);
//! ```
//!
//! # Supported event actions
//!
//! The provider dispatches an action based on the `event_action` field
//! of the payload:
//!
//! | `event_action` | API endpoint | Required fields |
//! |---|---|---|
//! | `"create"` | `POST /v2/alerts` | `message` |
//! | `"acknowledge"` | `POST /v2/alerts/{alias}/acknowledge` | `alias` |
//! | `"close"` | `POST /v2/alerts/{alias}/close` | `alias` |
//!
//! All three map naturally onto Alertmanager firing → acknowledged →
//! resolved state transitions so existing runbook tooling keeps
//! working after a migration.

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::{OpsGenieConfig, OpsGenieRegion};
pub use error::OpsGenieError;
pub use provider::OpsGenieProvider;
pub use types::{
    OpsGenieAlertRequest, OpsGenieApiResponse, OpsGenieLifecycleRequest, OpsGenieResponder,
};
