//! `VictorOps` / Splunk On-Call provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider) trait
//! against the [`VictorOps` REST endpoint integration][integration] — the
//! same endpoint Alertmanager targets via its `victorops_configs`, so a
//! migration is a matter of re-authoring the routing, not re-plumbing
//! the receiver.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_victorops::{VictorOpsConfig, VictorOpsProvider};
//!
//! // Single routing key (the common case).
//! let config = VictorOpsConfig::single_route(
//!     "organization-api-key",
//!     "team-ops",
//!     "team-ops-routing-key",
//! )
//! .with_monitoring_tool("acteon");
//! let provider = VictorOpsProvider::new(config);
//! ```
//!
//! Multiple routing keys are supported for deployments that fan alerts
//! out to different `VictorOps` teams based on the payload's
//! `routing_key` field.
//!
//! # Supported event actions
//!
//! The provider selects a `VictorOps` `message_type` based on the
//! payload's `event_action` field:
//!
//! | `event_action` | `VictorOps` `message_type` | Purpose |
//! |---|---|---|
//! | `"trigger"` | `CRITICAL` | Firing alert |
//! | `"warn"` | `WARNING` | Lower-priority alert |
//! | `"info"` | `INFO` | Informational (does not page) |
//! | `"acknowledge"` | `ACKNOWLEDGEMENT` | Oncall picked up |
//! | `"resolve"` | `RECOVERY` | Incident closed |
//!
//! All five map naturally onto Alertmanager firing → acknowledged →
//! resolved state transitions.
//!
//! [integration]: https://help.victorops.com/knowledge-base/rest-endpoint-integration-guide/

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::{DEFAULT_MONITORING_TOOL, VictorOpsConfig};
pub use error::VictorOpsError;
pub use provider::VictorOpsProvider;
pub use types::{VictorOpsAlertRequest, VictorOpsApiResponse, VictorOpsMessageType};
