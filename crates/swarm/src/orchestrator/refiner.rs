use std::process::Stdio;

use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::roles::RoleRegistry;
use crate::types::plan::{SwarmPlan, SwarmTask};

/// Result of plan refinement after a task completes.
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

/// Run the plan refiner after a task completes.
///
/// Invokes `claude -p --model haiku` with a short analysis prompt.
/// The refiner can add, skip, or reprioritize remaining tasks.
/// Non-fatal: any failure defaults to `Continue`.
pub async fn refine_plan(
    config: &SwarmConfig,
    roles: &RoleRegistry,
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

    if remaining.is_empty() {
        return Ok(RefinementAction::Continue);
    }

    let remaining_desc: Vec<String> = remaining
        .iter()
        .map(|t| {
            format!(
                "- {} (role: {}, priority: {}): {}",
                t.id, t.assigned_role, t.priority, t.description
            )
        })
        .collect();

    // Build delegation rules from role registry.
    let delegation_rules: Vec<String> = roles
        .all()
        .filter(|r| !r.can_delegate_to.is_empty())
        .map(|r| {
            format!(
                "- {} can delegate to: {}",
                r.name,
                r.can_delegate_to.join(", ")
            )
        })
        .collect();

    let delegation_section = if delegation_rules.is_empty() {
        String::new()
    } else {
        format!(
            "\n## Delegation Rules\n{}\nYou may suggest adding tasks with these delegated roles.\n",
            delegation_rules.join("\n")
        )
    };

    // Truncate output to keep prompt small.
    let truncated_output: String = subtask_output.chars().take(3000).collect();

    let prompt = format!(
        r"You are a plan refinement assistant. A task just completed in a multi-agent swarm. Evaluate whether the remaining plan should be adjusted.

## Completed Task
ID: {completed_task_id}

## Task Output (truncated)
{truncated_output}

## Remaining Tasks
{remaining}
{delegation_section}
## Instructions
Analyze the output. Respond with EXACTLY ONE of these lines:
1. CONTINUE
2. SKIP: task-id1, task-id2
3. REPRIORITIZE: task-id1=1, task-id2=5

Respond with ONLY the action line. No explanation.",
        remaining = remaining_desc.join("\n"),
    );

    let engine = config.defaults.engine;
    let cmd_name = match engine {
        crate::config::AgentEngine::Claude => "claude",
        crate::config::AgentEngine::Gemini => "gemini",
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::process::Command::new(cmd_name)
            .arg("-p")
            .arg(&prompt)
            .arg("--model")
            .arg(if matches!(engine, crate::config::AgentEngine::Claude) { "haiku" } else { "flash" })
            .arg("--output-format")
            .arg("text")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;

    let output = match result {
        Ok(Ok(o)) if o.status.success() => o,
        Ok(Ok(_)) => return Ok(RefinementAction::Continue),
        Ok(Err(e)) => {
            tracing::debug!("refiner spawn failed: {e}");
            return Ok(RefinementAction::Continue);
        }
        Err(_) => {
            tracing::debug!("refiner timed out");
            return Ok(RefinementAction::Continue);
        }
    };

    let response = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if response.is_empty() {
        return Ok(RefinementAction::Continue);
    }

    Ok(parse_refinement_response(&response, config))
}

/// Parse the refiner's text response into a `RefinementAction`.
fn parse_refinement_response(response: &str, _config: &SwarmConfig) -> RefinementAction {
    // Find the first non-empty line.
    let line = response
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("CONTINUE");

    if line.eq_ignore_ascii_case("CONTINUE") {
        return RefinementAction::Continue;
    }

    if let Some(ids) = line
        .strip_prefix("SKIP:")
        .or_else(|| line.strip_prefix("SKIP "))
    {
        let task_ids: Vec<String> = ids
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !task_ids.is_empty() {
            return RefinementAction::SkipTasks(task_ids);
        }
    }

    if let Some(pairs) = line
        .strip_prefix("REPRIORITIZE:")
        .or_else(|| line.strip_prefix("REPRIORITIZE "))
    {
        let priorities: Vec<(String, u32)> = pairs
            .split(',')
            .filter_map(|p| {
                let mut parts = p.trim().splitn(2, '=');
                let id = parts.next()?.trim().to_string();
                let priority = parts.next()?.trim().parse::<u32>().ok()?;
                Some((id, priority))
            })
            .collect();
        if !priorities.is_empty() {
            return RefinementAction::Reprioritize(priorities);
        }
    }

    RefinementAction::Continue
}

/// Apply a refinement action to the plan's task list.
///
/// Note: `SkipTasks` marks tasks as `Skipped` (by removing them from the plan)
/// rather than `Failed`. The caller should add skipped task IDs to
/// `completed_tasks` so dependent tasks can proceed.
pub fn apply_refinement(plan: &mut SwarmPlan, action: &RefinementAction) -> Vec<String> {
    match action {
        RefinementAction::Continue => vec![],
        RefinementAction::AddTasks(tasks) => {
            plan.tasks.extend(tasks.iter().cloned());
            vec![]
        }
        RefinementAction::SkipTasks(ids) => {
            plan.tasks.retain(|t| !ids.contains(&t.id));
            ids.clone()
        }
        RefinementAction::Reprioritize(priorities) => {
            for (id, priority) in priorities {
                if let Some(task) = plan.tasks.iter_mut().find(|t| &t.id == id) {
                    task.priority = *priority;
                }
            }
            vec![]
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
    fn test_parse_continue() {
        let config = SwarmConfig::minimal();
        assert!(matches!(
            parse_refinement_response("CONTINUE", &config),
            RefinementAction::Continue
        ));
        assert!(matches!(
            parse_refinement_response("continue", &config),
            RefinementAction::Continue
        ));
        assert!(matches!(
            parse_refinement_response("unknown garbage", &config),
            RefinementAction::Continue
        ));
    }

    #[test]
    fn test_parse_skip() {
        let config = SwarmConfig::minimal();
        match parse_refinement_response("SKIP: t1, t2", &config) {
            RefinementAction::SkipTasks(ids) => {
                assert_eq!(ids, vec!["t1", "t2"]);
            }
            other => panic!("expected SkipTasks, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_reprioritize() {
        let config = SwarmConfig::minimal();
        match parse_refinement_response("REPRIORITIZE: t1=5, t2=1", &config) {
            RefinementAction::Reprioritize(p) => {
                assert_eq!(p, vec![("t1".into(), 5), ("t2".into(), 1)]);
            }
            other => panic!("expected Reprioritize, got {other:?}"),
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
