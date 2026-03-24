use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::types::plan::{SwarmPlan, SwarmTask};

/// Result of plan refinement after a subtask completes.
#[derive(Debug, Clone)]
pub enum RefinementAction {
    /// No changes needed, continue as planned.
    Continue,
    /// Add new tasks to the plan.
    AddTasks(Vec<SwarmTask>),
    /// Skip specified task IDs (no longer needed).
    SkipTasks(Vec<String>),
    /// Reorder remaining tasks (provide new priority assignments).
    Reprioritize(Vec<(String, u32)>),
}

/// Run the plan refiner after a subtask completes.
///
/// Spawns a short `claude -p` session to evaluate the subtask result
/// and recommend plan adjustments. The refiner is read-only — it
/// analyzes but does not modify the codebase.
pub async fn refine_plan(
    _config: &SwarmConfig,
    plan: &SwarmPlan,
    completed_task_id: &str,
    subtask_output: &str,
    completed_tasks: &[&str],
) -> Result<RefinementAction, SwarmError> {
    let remaining: Vec<&SwarmTask> = plan
        .tasks
        .iter()
        .filter(|t| !completed_tasks.contains(&t.id.as_str()) && t.id != completed_task_id)
        .collect();

    // If no remaining tasks, nothing to refine.
    if remaining.is_empty() {
        return Ok(RefinementAction::Continue);
    }

    let remaining_desc: Vec<String> = remaining
        .iter()
        .map(|t| format!("- {} ({}): {}", t.id, t.name, t.description))
        .collect();

    let _prompt = format!(
        r#"You are a plan refinement assistant. A subtask just completed. Evaluate whether the remaining plan should be adjusted.

## Completed Task
ID: {completed_task_id}

## Subtask Output (summary)
{subtask_output}

## Remaining Tasks
{remaining}

## Instructions
Analyze the output. Respond with ONE of:
1. "CONTINUE" — no changes needed
2. "SKIP: task-id1, task-id2" — skip tasks that are no longer necessary
3. "ADD: <JSON array of new SwarmTask objects>" — add recovery/follow-up tasks
4. "REPRIORITIZE: task-id1=1, task-id2=5" — change execution order

Respond with ONLY the action line, nothing else."#,
        remaining = remaining_desc.join("\n"),
    );

    // TODO: invoke claude -p with the refinement prompt and parse response.
    // For now, return Continue as the default (no refinement).
    // This will be wired up in Phase 7 alongside the plan gathering.
    Ok(RefinementAction::Continue)
}

/// Apply a refinement action to the plan's task list.
pub fn apply_refinement(plan: &mut SwarmPlan, action: &RefinementAction) {
    match action {
        RefinementAction::Continue => {}
        RefinementAction::AddTasks(tasks) => {
            plan.tasks.extend(tasks.iter().cloned());
        }
        RefinementAction::SkipTasks(ids) => {
            plan.tasks.retain(|t| !ids.contains(&t.id));
        }
        RefinementAction::Reprioritize(priorities) => {
            for (id, priority) in priorities {
                if let Some(task) = plan.tasks.iter_mut().find(|t| &t.id == id) {
                    task.priority = *priority;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::plan::{SwarmScope, SwarmSubtask};
    use chrono::Utc;

    fn make_plan() -> SwarmPlan {
        SwarmPlan {
            id: "test".into(),
            objective: "test".into(),
            scope: SwarmScope {
                working_directory: "/tmp".into(),
                allowed_paths: vec![],
                forbidden_patterns: vec![],
                max_agents: 5,
                max_duration_minutes: 60,
                require_approval_for: vec![],
            },
            success_criteria: vec![],
            tasks: vec![
                SwarmTask {
                    id: "t1".into(),
                    name: "first".into(),
                    description: "first task".into(),
                    assigned_role: "coder".into(),
                    subtasks: vec![SwarmSubtask {
                        id: "s1".into(),
                        name: "sub".into(),
                        description: "do it".into(),
                        prompt: "do it".into(),
                        allowed_tools: None,
                        timeout_seconds: 60,
                    }],
                    depends_on: vec![],
                    priority: 1,
                },
                SwarmTask {
                    id: "t2".into(),
                    name: "second".into(),
                    description: "second task".into(),
                    assigned_role: "coder".into(),
                    subtasks: vec![],
                    depends_on: vec!["t1".into()],
                    priority: 2,
                },
            ],
            agent_roles: vec!["coder".into()],
            estimated_actions: 10,
            created_at: Utc::now(),
            approved_at: None,
        }
    }

    #[test]
    fn test_apply_skip_tasks() {
        let mut plan = make_plan();
        apply_refinement(&mut plan, &RefinementAction::SkipTasks(vec!["t2".into()]));
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].id, "t1");
    }

    #[test]
    fn test_apply_reprioritize() {
        let mut plan = make_plan();
        apply_refinement(
            &mut plan,
            &RefinementAction::Reprioritize(vec![("t2".into(), 0)]),
        );
        assert_eq!(plan.tasks[1].priority, 0);
    }

    #[test]
    fn test_apply_continue_is_noop() {
        let mut plan = make_plan();
        let original_len = plan.tasks.len();
        apply_refinement(&mut plan, &RefinementAction::Continue);
        assert_eq!(plan.tasks.len(), original_len);
    }
}
