//! Acteon provider that runs long-lived agent-swarm goals without blocking the
//! dispatch pipeline.
//!
//! A goal is dispatched as an `Action` with `provider = "swarm"` and a payload
//! containing a pre-built [`acteon_swarm::SwarmPlan`]. The provider accepts the
//! goal, spawns a background task, and returns immediately with a
//! `ProviderResponse` carrying the new `run_id`. Status is queried through a
//! separate API surface.
//!
//! The crate is deliberately free of server wiring so it can be reused by the
//! CLI, tests, and future transports without pulling in axum.

pub mod error;
pub mod executor;
pub mod provider;
pub mod registry;
pub mod sink;
pub mod types;

pub use error::SwarmProviderError;
pub use executor::{DefaultSwarmExecutor, SwarmExecutor};
pub use provider::SwarmProvider;
pub use registry::{SwarmRunHandle, SwarmRunRegistry};
pub use sink::{CompletionSink, LoggingSink, NoopSink};
pub use types::{GoalRequest, SwarmGoalAccepted, SwarmRunFilter, SwarmRunSnapshot, SwarmRunStatus};
