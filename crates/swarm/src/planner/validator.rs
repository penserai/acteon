use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use crate::error::SwarmError;
use crate::types::plan::SwarmPlan;

/// Warnings produced during plan validation (non-fatal).
#[derive(Debug, Clone)]
pub struct PlanWarning {
    pub message: String,
}

/// Validate a swarm plan for structural correctness.
///
/// Returns a list of non-fatal warnings. Raises [`SwarmError`] for fatal issues.
pub fn validate_plan<S: BuildHasher>(
    plan: &SwarmPlan,
    known_roles: &HashSet<String, S>,
) -> Result<Vec<PlanWarning>, SwarmError> {
    let mut warnings = Vec::new();

    // Collect all task IDs for reference checking.
    let task_ids: HashSet<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();

    // Check for duplicate task IDs.
    if task_ids.len() != plan.tasks.len() {
        return Err(SwarmError::PlanValidation(
            "duplicate task IDs detected".into(),
        ));
    }

    // Check that all dependency references are valid.
    for task in &plan.tasks {
        for dep in &task.depends_on {
            if !task_ids.contains(dep.as_str()) {
                return Err(SwarmError::PlanValidation(format!(
                    "task '{}' depends on unknown task '{dep}'",
                    task.id
                )));
            }
            if dep == &task.id {
                return Err(SwarmError::PlanValidation(format!(
                    "task '{}' depends on itself",
                    task.id
                )));
            }
        }
    }

    // Check for dependency cycles via DFS.
    detect_cycles(plan)?;

    // Check that all referenced roles exist.
    for task in &plan.tasks {
        if !known_roles.contains(&task.assigned_role) {
            return Err(SwarmError::UnknownRole(task.assigned_role.clone()));
        }
    }

    // Check that every task has at least one subtask.
    for task in &plan.tasks {
        if task.subtasks.is_empty() {
            warnings.push(PlanWarning {
                message: format!("task '{}' has no subtasks", task.id),
            });
        }
    }

    // Check for duplicate subtask IDs.
    let mut subtask_ids = HashSet::new();
    for task in &plan.tasks {
        for subtask in &task.subtasks {
            if !subtask_ids.insert(&subtask.id) {
                return Err(SwarmError::PlanValidation(format!(
                    "duplicate subtask ID: '{}'",
                    subtask.id
                )));
            }
        }
    }

    // Check estimated actions is reasonable.
    if plan.estimated_actions == 0 {
        warnings.push(PlanWarning {
            message: "estimated_actions is 0, quota will be very small".into(),
        });
    }

    Ok(warnings)
}

/// Detect cycles in the task dependency graph using DFS.
fn detect_cycles(plan: &SwarmPlan) -> Result<(), SwarmError> {
    // Build adjacency list: task_id -> [dependency_ids].
    let adj: HashMap<&str, Vec<&str>> = plan
        .tasks
        .iter()
        .map(|t| {
            (
                t.id.as_str(),
                t.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    for task in &plan.tasks {
        if !visited.contains(task.id.as_str()) {
            let mut path = Vec::new();
            if has_cycle(
                &adj,
                task.id.as_str(),
                &mut visited,
                &mut in_stack,
                &mut path,
            ) {
                return Err(SwarmError::DependencyCycle(
                    path.into_iter().map(String::from).collect(),
                ));
            }
        }
    }

    Ok(())
}

fn has_cycle<'a>(
    adj: &HashMap<&'a str, Vec<&'a str>>,
    node: &'a str,
    visited: &mut HashSet<&'a str>,
    in_stack: &mut HashSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> bool {
    visited.insert(node);
    in_stack.insert(node);
    path.push(node);

    if let Some(deps) = adj.get(node) {
        for &dep in deps {
            if !visited.contains(dep) {
                if has_cycle(adj, dep, visited, in_stack, path) {
                    return true;
                }
            } else if in_stack.contains(dep) {
                path.push(dep);
                return true;
            }
        }
    }

    in_stack.remove(node);
    path.pop();
    false
}

/// Build a topological ordering of tasks (dependencies first).
///
/// Returns task IDs in execution order. Panics if cycles exist
/// (call [`validate_plan`] first).
pub fn topological_sort(plan: &SwarmPlan) -> Vec<&str> {
    let adj: HashMap<&str, Vec<&str>> = plan
        .tasks
        .iter()
        .map(|t| {
            (
                t.id.as_str(),
                t.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();

    let mut visited = HashSet::new();
    let mut order = Vec::new();

    for task in &plan.tasks {
        if !visited.contains(task.id.as_str()) {
            topo_dfs(&adj, task.id.as_str(), &mut visited, &mut order);
        }
    }

    order
}

fn topo_dfs<'a>(
    adj: &HashMap<&'a str, Vec<&'a str>>,
    node: &'a str,
    visited: &mut HashSet<&'a str>,
    order: &mut Vec<&'a str>,
) {
    visited.insert(node);
    if let Some(deps) = adj.get(node) {
        for &dep in deps {
            if !visited.contains(dep) {
                topo_dfs(adj, dep, visited, order);
            }
        }
    }
    order.push(node);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::plan::{SwarmScope, SwarmSubtask, SwarmTask};
    use chrono::Utc;

    fn make_task(id: &str, deps: Vec<&str>) -> SwarmTask {
        SwarmTask {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            assigned_role: "coder".into(),
            subtasks: vec![SwarmSubtask {
                id: format!("{id}-sub"),
                name: "sub".into(),
                description: String::new(),
                prompt: "do it".into(),
                allowed_tools: None,
                timeout_seconds: 60,
            }],
            depends_on: deps.into_iter().map(String::from).collect(),
            priority: 10,
        }
    }

    fn make_plan(tasks: Vec<SwarmTask>) -> SwarmPlan {
        SwarmPlan {
            id: "test-plan".into(),
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
            tasks,
            agent_roles: vec!["coder".into()],
            estimated_actions: 10,
            created_at: Utc::now(),
            approved_at: None,
        }
    }

    fn roles() -> HashSet<String> {
        ["coder", "researcher", "reviewer", "executor", "planner"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    #[test]
    fn test_valid_linear_plan() {
        let plan = make_plan(vec![
            make_task("a", vec![]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ]);
        let warnings = validate_plan(&plan, &roles()).unwrap();
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_cycle_detected() {
        let plan = make_plan(vec![
            make_task("a", vec!["c"]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["b"]),
        ]);
        let result = validate_plan(&plan, &roles());
        assert!(matches!(result, Err(SwarmError::DependencyCycle(_))));
    }

    #[test]
    fn test_self_dependency() {
        let plan = make_plan(vec![make_task("a", vec!["a"])]);
        let result = validate_plan(&plan, &roles());
        assert!(matches!(result, Err(SwarmError::PlanValidation(_))));
    }

    #[test]
    fn test_unknown_dependency() {
        let plan = make_plan(vec![make_task("a", vec!["nonexistent"])]);
        let result = validate_plan(&plan, &roles());
        assert!(matches!(result, Err(SwarmError::PlanValidation(_))));
    }

    #[test]
    fn test_unknown_role() {
        let mut plan = make_plan(vec![make_task("a", vec![])]);
        plan.tasks[0].assigned_role = "wizard".into();
        let result = validate_plan(&plan, &roles());
        assert!(matches!(result, Err(SwarmError::UnknownRole(_))));
    }

    #[test]
    fn test_duplicate_task_ids() {
        let plan = make_plan(vec![make_task("a", vec![]), make_task("a", vec![])]);
        let result = validate_plan(&plan, &roles());
        assert!(matches!(result, Err(SwarmError::PlanValidation(_))));
    }

    #[test]
    fn test_empty_subtasks_warning() {
        let mut plan = make_plan(vec![make_task("a", vec![])]);
        plan.tasks[0].subtasks.clear();
        let warnings = validate_plan(&plan, &roles()).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("no subtasks"));
    }

    #[test]
    fn test_topological_sort_linear() {
        let plan = make_plan(vec![
            make_task("c", vec!["b"]),
            make_task("b", vec!["a"]),
            make_task("a", vec![]),
        ]);
        let order = topological_sort(&plan);
        let a_pos = order.iter().position(|&x| x == "a").unwrap();
        let b_pos = order.iter().position(|&x| x == "b").unwrap();
        let c_pos = order.iter().position(|&x| x == "c").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let plan = make_plan(vec![
            make_task("d", vec!["b", "c"]),
            make_task("b", vec!["a"]),
            make_task("c", vec!["a"]),
            make_task("a", vec![]),
        ]);
        let order = topological_sort(&plan);
        let a_pos = order.iter().position(|&x| x == "a").unwrap();
        let b_pos = order.iter().position(|&x| x == "b").unwrap();
        let c_pos = order.iter().position(|&x| x == "c").unwrap();
        let d_pos = order.iter().position(|&x| x == "d").unwrap();
        assert!(a_pos < b_pos);
        assert!(a_pos < c_pos);
        assert!(b_pos < d_pos);
        assert!(c_pos < d_pos);
    }
}
