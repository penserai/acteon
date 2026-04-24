//! End-to-end exercise of the ambient `swarm` provider.
//!
//! This simulation wires a `SwarmProvider` backed by a **mocked** executor
//! (so the example runs offline and fast) into an in-memory gateway, then:
//!
//! 1. Dispatches three goals as Actions with `provider = "swarm"`.
//! 2. Demonstrates the fire-and-forget contract — the dispatch returns
//!    immediately with a `SwarmGoalAccepted` body carrying a `run_id`.
//! 3. Subscribes to the registry's broadcast channel and observes each
//!    run's `accepted → running → completed` transitions.
//! 4. Cancels one inflight run and verifies it transitions through
//!    `cancelling → cancelled`.
//!
//! Run with:
//! ```text
//! cargo run -p acteon-simulation --features swarm --example swarm_provider_simulation
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::info;

use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use acteon_swarm::SwarmPlan;
use acteon_swarm::types::plan::SwarmScope;
use acteon_swarm::types::run::{RunMetrics, SwarmRun, SwarmRunStatus as InnerRunStatus};
use acteon_swarm_provider::{
    GoalRequest, LoggingSink, SwarmExecutor, SwarmProvider, SwarmProviderError, SwarmRunFilter,
    SwarmRunRegistry, SwarmRunStatus,
};

/// Mock executor that simulates work by sleeping, then returns success.
struct MockExecutor {
    delay: Duration,
}

#[async_trait]
impl SwarmExecutor for MockExecutor {
    async fn run(&self, plan: SwarmPlan) -> Result<SwarmRun, SwarmProviderError> {
        tokio::time::sleep(self.delay).await;
        Ok(SwarmRun {
            id: uuid::Uuid::new_v4().to_string(),
            plan_id: plan.id,
            status: InnerRunStatus::Completed,
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            task_status: std::collections::HashMap::new(),
            metrics: RunMetrics {
                total_actions: 12,
                agents_spawned: 3,
                agents_completed: 3,
                ..RunMetrics::default()
            },
        })
    }
}

fn mock_plan(objective: &str) -> SwarmPlan {
    SwarmPlan {
        id: format!("plan-{}", uuid::Uuid::new_v4()),
        objective: objective.to_string(),
        scope: SwarmScope::default(),
        success_criteria: vec![],
        tasks: vec![],
        agent_roles: vec![],
        estimated_actions: 0,
        created_at: chrono::Utc::now(),
        approved_at: None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info,acteon_swarm_provider=debug")
        .try_init()
        .ok();

    // --- Registry + provider wired together ----------------------------------
    let registry = SwarmRunRegistry::new(
        Arc::new(MockExecutor {
            delay: Duration::from_millis(100),
        }),
        Arc::new(LoggingSink),
        4,
    );
    let provider: Arc<dyn acteon_provider::DynProvider> =
        Arc::new(SwarmProvider::new("swarm", registry.clone()));

    // Subscribe BEFORE dispatching so no transitions are lost.
    let mut updates = registry.subscribe();

    // --- Gateway ------------------------------------------------------------
    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .provider(Arc::clone(&provider))
        .build()?;

    // --- Dispatch three goals -----------------------------------------------
    let goals = [
        "sweep stale PRs",
        "investigate flaky test",
        "summarize last week's incidents",
    ];
    let mut run_ids = Vec::new();
    for objective in goals {
        let payload = serde_json::to_value(GoalRequest {
            objective: objective.to_string(),
            plan: mock_plan(objective),
            idempotency_key: None,
        })?;
        let action = Action::new("research", "acme", "swarm", "swarm.goal", payload);
        let outcome = gateway.dispatch(action, None).await?;
        info!(?outcome, "dispatch returned immediately");
        if let Some(body) = extract_body(&outcome) {
            run_ids.push(body["run_id"].as_str().unwrap().to_string());
        }
    }
    info!(count = run_ids.len(), "goals accepted");

    // --- Observe transitions ------------------------------------------------
    let listener = tokio::spawn(async move {
        let mut completed = 0usize;
        while completed < 3 {
            if let Ok(snap) = tokio::time::timeout(Duration::from_secs(2), updates.recv()).await {
                match snap {
                    Ok(s) => {
                        info!(run_id = %s.run_id, status = ?s.status, "transition");
                        if s.status == SwarmRunStatus::Completed
                            || s.status == SwarmRunStatus::Cancelled
                        {
                            completed += 1;
                        }
                    }
                    Err(_) => break,
                }
            } else {
                break;
            }
        }
    });

    // --- Cancel the second run while still inflight -------------------------
    if let Some(target) = run_ids.get(1).cloned() {
        // Give the run a moment to flip to Running.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let snap = registry.cancel(&target).await?;
        info!(run_id = %target, status = ?snap.status, "cancellation requested");
    }

    // Wait for observers to catch terminal transitions.
    let _ = tokio::time::timeout(Duration::from_secs(5), listener).await;

    // --- Final listing ------------------------------------------------------
    let (snapshots, total) = registry.list(&SwarmRunFilter::default());
    info!(total, "final snapshot count");
    for s in snapshots {
        info!(
            run_id = %s.run_id,
            status = ?s.status,
            objective = %s.objective,
            "final snapshot"
        );
    }

    Ok(())
}

fn extract_body(outcome: &acteon_core::ActionOutcome) -> Option<serde_json::Value> {
    match outcome {
        acteon_core::ActionOutcome::Executed(resp) => Some(resp.body.clone()),
        _ => None,
    }
}
