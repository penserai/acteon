use std::sync::Arc;

use acteon_core::{Action, ProviderResponse};
use acteon_provider::{Provider, ProviderError};

use crate::registry::SwarmRunRegistry;
use crate::types::GoalRequest;

/// Provider name used when dispatching swarm goals.
pub const SWARM_PROVIDER_NAME: &str = "swarm";

/// Provider that accepts swarm goals and hands them off to the registry.
///
/// The provider itself is stateless beyond the registry handle — all long-lived
/// work lives on the registry so the HTTP API can share the same state.
pub struct SwarmProvider {
    name: String,
    registry: Arc<SwarmRunRegistry>,
}

impl SwarmProvider {
    #[must_use]
    pub fn new(name: impl Into<String>, registry: Arc<SwarmRunRegistry>) -> Self {
        Self {
            name: name.into(),
            registry,
        }
    }

    #[must_use]
    pub fn registry(&self) -> Arc<SwarmRunRegistry> {
        self.registry.clone()
    }
}

impl Provider for SwarmProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        let request: GoalRequest = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(format!("invalid swarm goal: {e}")))?;

        let accepted = self
            .registry
            .start(
                action.namespace.to_string(),
                action.tenant.to_string(),
                request,
            )
            .await
            .map_err(|e| ProviderError::ExecutionFailed(e.to_string()))?;

        let body = serde_json::to_value(&accepted)
            .map_err(|e| ProviderError::ExecutionFailed(format!("serialize accepted: {e}")))?;
        Ok(ProviderResponse::success(body))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        // Always healthy — the registry is an in-process data structure; if
        // the executor is broken, individual runs will flip to Failed.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::SwarmExecutor;
    use crate::sink::NoopSink;
    use crate::types::SwarmGoalAccepted;
    use acteon_swarm::SwarmPlan;
    use acteon_swarm::types::plan::SwarmScope;
    use acteon_swarm::types::run::{RunMetrics, SwarmRun, SwarmRunStatus as InnerStatus};
    use async_trait::async_trait;

    struct ImmediateExecutor;

    #[async_trait]
    impl SwarmExecutor for ImmediateExecutor {
        async fn run(&self, plan: SwarmPlan) -> Result<SwarmRun, crate::error::SwarmProviderError> {
            Ok(SwarmRun {
                id: uuid::Uuid::new_v4().to_string(),
                plan_id: plan.id,
                status: InnerStatus::Completed,
                started_at: chrono::Utc::now(),
                finished_at: Some(chrono::Utc::now()),
                task_status: std::collections::HashMap::new(),
                metrics: RunMetrics::default(),
            })
        }
    }

    fn sample_plan() -> SwarmPlan {
        SwarmPlan {
            id: "plan-1".into(),
            objective: "demo".into(),
            scope: SwarmScope::default(),
            success_criteria: vec![],
            tasks: vec![],
            agent_roles: vec![],
            estimated_actions: 0,
            created_at: chrono::Utc::now(),
            approved_at: None,
        }
    }

    #[tokio::test]
    async fn execute_returns_accepted() {
        let registry = SwarmRunRegistry::new(Arc::new(ImmediateExecutor), Arc::new(NoopSink), 4);
        let provider = SwarmProvider::new("swarm", registry.clone());
        let payload = serde_json::to_value(GoalRequest {
            objective: "demo".into(),
            plan: sample_plan(),
            idempotency_key: None,
        })
        .unwrap();
        let action = Action::new("ns", "t", "swarm", "swarm.goal", payload);

        let resp = provider.execute(&action).await.unwrap();
        assert_eq!(resp.status, acteon_core::ResponseStatus::Success);
        let accepted: SwarmGoalAccepted = serde_json::from_value(resp.body).unwrap();
        assert_eq!(accepted.plan_id, "plan-1");
        assert!(!accepted.run_id.is_empty());
    }

    #[tokio::test]
    async fn execute_rejects_invalid_payload() {
        let registry = SwarmRunRegistry::new(Arc::new(ImmediateExecutor), Arc::new(NoopSink), 4);
        let provider = SwarmProvider::new("swarm", registry);
        let action = Action::new(
            "ns",
            "t",
            "swarm",
            "swarm.goal",
            serde_json::json!({"bogus": true}),
        );
        let err = provider.execute(&action).await.unwrap_err();
        assert!(matches!(err, ProviderError::Serialization(_)));
    }
}
