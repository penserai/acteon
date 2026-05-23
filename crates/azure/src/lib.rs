//! Azure service providers for the Acteon action gateway.
//!
//! This crate provides feature-gated integrations with Azure services:
//!
//! - **Blob Storage** (`blob` feature) — Upload/download/delete blobs
//! - **Event Hubs** (`eventhubs` feature) — Send events and batches
//!
//! All providers share a common [`AzureBaseConfig`] for
//! location, endpoint override, and optional service principal credentials.

pub mod auth;
pub mod config;
pub mod error;

#[cfg(feature = "blob")]
pub mod blob;

#[cfg(feature = "eventhubs")]
pub mod eventhubs;

// Re-exports for convenience.
pub use config::AzureBaseConfig;
pub use error::AzureProviderError;

#[cfg(feature = "blob")]
pub use blob::{BlobConfig, BlobProvider};

#[cfg(feature = "eventhubs")]
pub use eventhubs::{EventHubsConfig, EventHubsProvider};
