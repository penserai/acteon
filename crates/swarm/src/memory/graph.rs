use serde_json::json;

use super::client::{CreateTwinRequest, TesseraiClient};
use crate::types::plan::SwarmPlan;
use crate::types::run::SwarmRun;

/// Build the full digital twin graph for a completed swarm run.
///
/// Creates `SwarmTask` twins and wires up relationships:
/// ```text
/// SwarmRun ──hasTask──→ SwarmTask ──assignedTo──→ AgentSession
///     │                     │                         │
///     │                     └──dependsOn──→ SwarmTask  ├──produced──→ EpisodicMemory
///     │                                               └──discovered──→ SemanticMemory
///     └──hasAgent──→ AgentSession
/// ```
pub async fn build_swarm_graph(
    client: &TesseraiClient,
    run_id: &str,
    plan: &SwarmPlan,
    run: &SwarmRun,
) {
    let run_twin_id = format!("swarm-run-{run_id}");

    // 1. Create SwarmTask twins and link run → task.
    for task in &plan.tasks {
        let task_twin_id = format!("swarm-task-{run_id}-{}", task.id);
        let status = run
            .task_status
            .get(&task.id)
            .map_or_else(|| "unknown".into(), |s| format!("{s:?}"));

        if let Err(e) = client
            .create_twin(&CreateTwinRequest {
                id: task_twin_id.clone(),
                twin_type: "SwarmTask".into(),
                name: Some(task.name.clone()),
                description: Some(task.description.clone()),
                properties: json!({
                    "task_id": task.id,
                    "run_id": run_id,
                    "assigned_role": task.assigned_role,
                    "priority": task.priority,
                    "status": status,
                    "subtask_count": task.subtasks.len(),
                    "depends_on": task.depends_on,
                }),
            })
            .await
        {
            tracing::debug!("failed to create task twin {}: {e}", task.id);
            continue;
        }

        // run → hasTask → task
        let _ = client
            .create_relationship(&run_twin_id, "hasTask", &task_twin_id)
            .await;

        // task → dependsOn → other tasks
        for dep_id in &task.depends_on {
            let dep_twin_id = format!("swarm-task-{run_id}-{dep_id}");
            let _ = client
                .create_relationship(&task_twin_id, "dependsOn", &dep_twin_id)
                .await;
        }
    }

    // 2. Link agents to their tasks and to the run.
    let all_twins = client
        .list_twins(None, None)
        .await
        .unwrap_or_default();

    let agents: Vec<_> = all_twins
        .iter()
        .filter(|t| t.id.starts_with("swarm-agent-"))
        .collect();

    for agent in &agents {
        // run → hasAgent → agent
        let _ = client
            .create_relationship(&run_twin_id, "hasAgent", &agent.id)
            .await;

        // Get full agent twin to find task_id
        let task_id = client
            .get_twin(&agent.id)
            .await
            .ok()
            .and_then(|full| {
                full.properties
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        if let Some(tid) = task_id {
            let task_twin_id = format!("swarm-task-{run_id}-{tid}");
            let _ = client
                .create_relationship(&task_twin_id, "assignedTo", &agent.id)
                .await;
        }
    }

    // 3. Link memories to their producing agents.
    let episodic_twins: Vec<_> = all_twins
        .iter()
        .filter(|t| t.id.starts_with("memory-"))
        .collect();
    let finding_twins: Vec<_> = all_twins
        .iter()
        .filter(|t| t.id.starts_with("finding-"))
        .collect();

    link_memories_to_agents(client, &episodic_twins, "produced").await;
    link_memories_to_agents(client, &finding_twins, "discovered").await;

    tracing::info!(
        tasks = plan.tasks.len(),
        agents = agents.len(),
        "built swarm digital twin graph"
    );
}

/// Link memory twins to their producing agent twins via a relationship.
async fn link_memories_to_agents(
    client: &TesseraiClient,
    memories: &[&super::client::TwinResponse],
    rel_type: &str,
) {
    for mem in memories {
        let agent_id = client
            .get_twin(&mem.id)
            .await
            .ok()
            .and_then(|full| {
                full.properties
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });
        if let Some(aid) = agent_id {
            let agent_twin_id = format!("swarm-agent-{aid}");
            let _ = client
                .create_relationship(&agent_twin_id, rel_type, &mem.id)
                .await;
        }
    }
}
