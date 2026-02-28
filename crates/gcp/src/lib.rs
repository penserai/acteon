//! GCP service providers for the Acteon action gateway.
//!
//! This crate provides feature-gated integrations with Google Cloud services:
//!
//! - **Pub/Sub** (`pubsub` feature) — Publish messages and batches
//! - **Cloud Storage** (`storage` feature) — Upload/download/delete objects
//!
//! All providers share a common [`GcpBaseConfig`] for
//! project ID, credentials path, and optional endpoint URL override.

pub mod auth;
pub mod config;
pub mod error;

#[cfg(feature = "pubsub")]
pub mod pubsub;

#[cfg(feature = "storage")]
pub mod storage;

// Re-exports for convenience.
pub use config::GcpBaseConfig;
pub use error::GcpProviderError;

#[cfg(feature = "pubsub")]
pub use pubsub::{PubSubConfig, PubSubProvider};

#[cfg(feature = "storage")]
pub use storage::{StorageConfig, StorageProvider};
