//! Cluster management for simulation testing.
//!
//! This module provides components for running multi-node Acteon clusters
//! in a local testing environment.

mod config;
mod node;
mod port_allocator;

pub use config::{AuditBackendConfig, ClusterConfig, SimulationConfig, StateBackendConfig};
pub use node::ServerNode;
pub use port_allocator::PortAllocator;
