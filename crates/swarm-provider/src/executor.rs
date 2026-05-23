use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use acteon_swarm::config::SwarmConfig;
use acteon_swarm::roles::RoleRegistry;
use acteon_swarm::{SwarmPlan, SwarmRun, execute_swarm};

use crate::error::SwarmProviderError;

/// Pluggable runner that owns the long path through a swarm plan.
///
/// Production uses [`DefaultSwarmExecutor`], which shells out to
/// `acteon_swarm::execute_swarm`. Tests substitute a stub so they don't need
/// to spin up real Claude Agent SDK processes.
#[async_trait]
pub trait SwarmExecutor: Send + Sync + 'static {
    async fn run(&self, plan: SwarmPlan) -> Result<SwarmRun, SwarmProviderError>;
}

/// Default executor that delegates to `acteon_swarm::execute_swarm`.
pub struct DefaultSwarmExecutor {
    config: SwarmConfig,
    roles: RoleRegistry,
    hooks_binary: PathBuf,
}

impl DefaultSwarmExecutor {
    #[must_use]
    pub fn new(config: SwarmConfig, roles: RoleRegistry, hooks_binary: PathBuf) -> Self {
        Self {
            config,
            roles,
            hooks_binary,
        }
    }
}

#[async_trait]
impl SwarmExecutor for DefaultSwarmExecutor {
    async fn run(&self, mut plan: SwarmPlan) -> Result<SwarmRun, SwarmProviderError> {
        execute_swarm(&mut plan, &self.config, &self.roles, &self.hooks_binary)
            .await
            .map_err(|e| SwarmProviderError::Executor(e.to_string()))
    }
}

/// Trait object alias for storing any executor behind an `Arc`.
pub type SharedExecutor = Arc<dyn SwarmExecutor>;
