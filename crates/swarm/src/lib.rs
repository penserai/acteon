//! `acteon-swarm` — Generic multi-agent swarm orchestrator.
//!
//! Orchestrates specialist agents via the Claude Agent SDK, using
//! Acteon for workflow orchestration and safety enforcement, and
//! `TesseraiDB` for shared knowledge and semantic memory.

pub mod acteon;
pub mod config;
pub mod error;
pub mod hooks;
pub mod memory;
pub mod orchestrator;
pub mod planner;
pub mod roles;
pub mod types;

pub use config::SwarmConfig;
pub use error::SwarmError;
pub use memory::TesseraiClient;
pub use orchestrator::{execute_swarm, execute_swarm_with_adversarial};
pub use planner::{topological_sort, validate_plan};
pub use roles::RoleRegistry;
pub use types::{
    AdversarialChallenge, AdversarialResult, AdversarialRound, AgentRole, AgentSession, SwarmPlan,
    SwarmRun,
};
