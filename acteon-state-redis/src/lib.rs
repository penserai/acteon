//! Redis state backend for Acteon.
//!
//! This crate provides Redis-backed implementations of the [`StateStore`] and
//! [`DistributedLock`] traits from `acteon-state`.
//!
//! # Features
//!
//! - **State storage**: Key-value operations with optional TTL using Redis strings.
//! - **Distributed locking**: Mutual exclusion using `SET NX PX` with Lua scripts.
//! - **Connection pooling**: Uses `deadpool-redis` for efficient connection management.
//!
//! # Lock Consistency
//!
//! The distributed lock implementation provides different guarantees depending
//! on your Redis deployment:
//!
//! | Deployment | Mutual Exclusion | Notes |
//! |------------|------------------|-------|
//! | Single instance | Strong | Full mutual exclusion guaranteed |
//! | Sentinel | Weak | Lock may be lost during failover |
//! | Cluster | Weak | Lock may be lost during failover |
//!
//! For applications requiring strong consistency during failovers, consider
//! using the `PostgreSQL` or `DynamoDB` backends instead.
//!
//! See [`lock`] module documentation for detailed information.
//!
//! # Example
//!
//! ```ignore
//! use acteon_state_redis::{RedisConfig, RedisStateStore, RedisDistributedLock};
//!
//! let config = RedisConfig::new("redis://localhost:6379");
//! let store = RedisStateStore::new(&config)?;
//! let lock = RedisDistributedLock::new(&config)?;
//! ```
//!
//! [`StateStore`]: acteon_state::StateStore
//! [`DistributedLock`]: acteon_state::DistributedLock

mod config;
mod key_render;
pub mod lock;
mod scripts;
mod store;

pub use config::RedisConfig;
pub use lock::RedisDistributedLock;
pub use store::RedisStateStore;
