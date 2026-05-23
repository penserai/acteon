use crate::types::agent::AgentRole;
use crate::types::plan::{SwarmSubtask, SwarmTask};

/// Build the system prompt for an agent from its role template and task context.
///
/// Uses `MiniJinja` to render the role's `system_prompt_template` with:
/// - `task.name`, `task.description`, `task.id`
/// - `subtask.name`, `subtask.description`, `subtask.id`, `subtask.prompt`
pub fn build_system_prompt(role: &AgentRole, task: &SwarmTask, subtask: &SwarmSubtask) -> String {
    let mut env = minijinja::Environment::new();
    env.add_template("prompt", &role.system_prompt_template)
        .unwrap_or_else(|e| {
            tracing::warn!("invalid prompt template for role {}: {e}", role.name);
        });

    let ctx = minijinja::context! {
        task => minijinja::context! {
            id => &task.id,
            name => &task.name,
            description => &task.description,
        },
        subtask => minijinja::context! {
            id => &subtask.id,
            name => &subtask.name,
            description => &subtask.description,
            prompt => &subtask.prompt,
        },
        role => minijinja::context! {
            name => &role.name,
            description => &role.description,
        },
    };

    match env.get_template("prompt") {
        Ok(tmpl) => tmpl.render(ctx).unwrap_or_else(|e| {
            tracing::warn!("prompt render failed for role {}: {e}", role.name);
            role.system_prompt_template.clone()
        }),
        Err(_) => role.system_prompt_template.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let role = AgentRole {
            name: "coder".into(),
            description: "Writes code".into(),
            system_prompt_template:
                "Task: {{ task.name }}\nSubtask: {{ subtask.name }}\nRole: {{ role.name }}".into(),
            allowed_tools: vec![],
            can_delegate_to: vec![],
            max_concurrent_instances: 1,
        };
        let task = SwarmTask {
            id: "t1".into(),
            name: "Build API".into(),
            description: "Create REST endpoints".into(),
            assigned_role: "coder".into(),
            subtasks: vec![],
            depends_on: vec![],
            priority: 1,
        };
        let subtask = SwarmSubtask {
            id: "s1".into(),
            name: "Create handler".into(),
            description: "Implement GET /users".into(),
            prompt: "Write the handler".into(),
            allowed_tools: None,
            timeout_seconds: 60,
        };

        let result = build_system_prompt(&role, &task, &subtask);
        assert_eq!(
            result,
            "Task: Build API\nSubtask: Create handler\nRole: coder"
        );
    }

    #[test]
    fn test_invalid_template_returns_raw() {
        let role = AgentRole {
            name: "test".into(),
            description: "test".into(),
            system_prompt_template: "{{ unclosed".into(),
            allowed_tools: vec![],
            can_delegate_to: vec![],
            max_concurrent_instances: 1,
        };
        let task = SwarmTask {
            id: "t1".into(),
            name: "test".into(),
            description: "test".into(),
            assigned_role: "test".into(),
            subtasks: vec![],
            depends_on: vec![],
            priority: 1,
        };
        let subtask = SwarmSubtask {
            id: "s1".into(),
            name: "test".into(),
            description: "test".into(),
            prompt: "test".into(),
            allowed_tools: None,
            timeout_seconds: 60,
        };

        // Should not panic, returns raw template.
        let result = build_system_prompt(&role, &task, &subtask);
        assert!(result.contains("unclosed"));
    }
}
