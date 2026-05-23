use serde_json::json;

use super::client::{CreateTwinRequest, TesseraiClient};
use crate::error::SwarmError;
use crate::types::agent::AgentSession;
use crate::types::plan::SwarmPlan;

/// Create a `TesseraiDB` twin representing a swarm run.
pub async fn create_run_twin(
    client: &TesseraiClient,
    run_id: &str,
    plan: &SwarmPlan,
) -> Result<(), SwarmError> {
    let task_ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();

    client
        .create_twin(&CreateTwinRequest {
            id: format!("swarm-run-{run_id}"),
            twin_type: "SwarmRun".into(),
            name: Some(format!("Swarm: {}", plan.objective)),
            description: Some(plan.objective.clone()),
            properties: json!({
                "run_id": run_id,
                "plan_id": plan.id,
                "objective": plan.objective,
                "status": "running",
                "started_at": chrono::Utc::now().to_rfc3339(),
                "task_ids": task_ids,
                "agent_roles": plan.agent_roles,
                "estimated_actions": plan.estimated_actions,
            }),
        })
        .await?;

    Ok(())
}

/// Create a `TesseraiDB` twin representing an agent session.
pub async fn create_session_twin(
    client: &TesseraiClient,
    run_id: &str,
    session: &AgentSession,
) -> Result<(), SwarmError> {
    client
        .create_twin(&CreateTwinRequest {
            id: format!("swarm-agent-{}", session.id),
            twin_type: "AgentSession".into(),
            name: Some(format!("{} agent ({})", session.role, session.subtask_id)),
            description: None,
            properties: json!({
                "session_id": session.id,
                "run_id": run_id,
                "role": session.role,
                "task_id": session.task_id,
                "subtask_id": session.subtask_id,
                "status": "running",
                "started_at": session.started_at.to_rfc3339(),
                "workspace": session.workspace.display().to_string(),
            }),
        })
        .await?;

    Ok(())
}

/// Update a run twin's status.
pub async fn update_run_status(
    client: &TesseraiClient,
    run_id: &str,
    status: &str,
) -> Result<(), SwarmError> {
    client
        .patch_twin(
            &format!("swarm-run-{run_id}"),
            &json!({
                "properties": {
                    "status": status,
                    "updated_at": chrono::Utc::now().to_rfc3339(),
                }
            }),
        )
        .await?;

    Ok(())
}

/// Update an agent session twin's status.
pub async fn update_session_status(
    client: &TesseraiClient,
    session: &AgentSession,
) -> Result<(), SwarmError> {
    let status_str = match &session.status {
        crate::types::agent::AgentSessionStatus::Pending => "pending",
        crate::types::agent::AgentSessionStatus::Running => "running",
        crate::types::agent::AgentSessionStatus::WaitingApproval => "waiting_approval",
        crate::types::agent::AgentSessionStatus::Completed => "completed",
        crate::types::agent::AgentSessionStatus::Failed(_) => "failed",
        crate::types::agent::AgentSessionStatus::TimedOut => "timed_out",
        crate::types::agent::AgentSessionStatus::Cancelled => "cancelled",
    };

    client
        .patch_twin(
            &format!("swarm-agent-{}", session.id),
            &json!({
                "properties": {
                    "status": status_str,
                    "updated_at": chrono::Utc::now().to_rfc3339(),
                    "actions_dispatched": session.actions_dispatched,
                    "actions_blocked": session.actions_blocked,
                }
            }),
        )
        .await?;

    Ok(())
}
