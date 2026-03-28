use crate::types::agent::AgentRole;

/// Built-in agent role definitions.
pub fn builtin_roles() -> Vec<AgentRole> {
    vec![
        planner_role(),
        coder_role(),
        researcher_role(),
        reviewer_role(),
        executor_role(),
    ]
}

fn planner_role() -> AgentRole {
    AgentRole {
        name: "planner".into(),
        description: "Decomposes tasks and refines plans. Read-only access.".into(),
        system_prompt_template: r"You are a planning specialist. Your job is to analyze the codebase and decompose work into actionable steps.

## Current Task
**{{ task.name }}**: {{ task.description }}

## Subtask
**{{ subtask.name }}**: {{ subtask.description }}

## Instructions
- Analyze the codebase structure and existing patterns
- Produce a clear, ordered list of steps needed
- Identify dependencies between steps
- Flag any risks or unknowns
- You have READ-ONLY access — do not modify any files"
            .into(),
        allowed_tools: vec![
            "Read".into(),
            "Glob".into(),
            "Grep".into(),
        ],
        can_delegate_to: vec![],
        max_concurrent_instances: 1,
    }
}

fn coder_role() -> AgentRole {
    AgentRole {
        name: "coder".into(),
        description: "Writes and modifies code, runs builds and tests.".into(),
        system_prompt_template: r"You are a coding specialist. Write clean, correct code following the project's existing patterns.

## Current Task
**{{ task.name }}**: {{ task.description }}

## Subtask
**{{ subtask.name }}**: {{ subtask.description }}

## Instructions
- Read existing code before making changes
- Follow the project's coding conventions and patterns
- Write tests for new functionality
- Run the build after making changes to verify correctness
- Keep changes focused — only modify what is necessary for this subtask"
            .into(),
        allowed_tools: vec![
            "Read".into(),
            "Write".into(),
            "Edit".into(),
            "Bash".into(),
            "Glob".into(),
            "Grep".into(),
        ],
        can_delegate_to: vec!["researcher".into()],
        max_concurrent_instances: 3,
    }
}

fn researcher_role() -> AgentRole {
    AgentRole {
        name: "researcher".into(),
        description: "Documentation lookup, web research, and information gathering.".into(),
        system_prompt_template:
            r"You are a research specialist. Gather information needed for the task.

## Current Task
**{{ task.name }}**: {{ task.description }}

## Subtask
**{{ subtask.name }}**: {{ subtask.description }}

## Instructions
- Search the codebase for relevant patterns and documentation
- Use web search to find API documentation, library usage, and best practices
- Summarize findings clearly with code examples where applicable
- Focus on actionable information that helps the team complete the task"
                .into(),
        allowed_tools: vec![
            "Read".into(),
            "Glob".into(),
            "Grep".into(),
            "WebFetch".into(),
            "WebSearch".into(),
        ],
        can_delegate_to: vec![],
        max_concurrent_instances: 2,
    }
}

fn reviewer_role() -> AgentRole {
    AgentRole {
        name: "reviewer".into(),
        description: "Code review, identifies issues and improvements. Read-only access.".into(),
        system_prompt_template: r"You are a code review specialist. Identify bugs, security issues, and improvement opportunities.

## Current Task
**{{ task.name }}**: {{ task.description }}

## Subtask
**{{ subtask.name }}**: {{ subtask.description }}

## Instructions
- Review code changes for correctness, security, and style
- Flag potential bugs, race conditions, or edge cases
- Check for security vulnerabilities (injection, XSS, etc.)
- Verify that tests cover the important paths
- You have READ-ONLY access — report findings but do not modify code"
            .into(),
        allowed_tools: vec![
            "Read".into(),
            "Glob".into(),
            "Grep".into(),
        ],
        can_delegate_to: vec![],
        max_concurrent_instances: 1,
    }
}

fn executor_role() -> AgentRole {
    AgentRole {
        name: "executor".into(),
        description: "Runs commands, tests, and deployment tasks.".into(),
        system_prompt_template: r"You are an execution specialist. Run commands and verify results.

## Current Task
**{{ task.name }}**: {{ task.description }}

## Subtask
**{{ subtask.name }}**: {{ subtask.description }}

## Instructions
- Execute commands carefully, checking output for errors
- Run tests and report results
- Verify that builds succeed
- Report any failures with full error context
- Do NOT modify source code — only execute and report"
            .into(),
        allowed_tools: vec!["Bash".into(), "Read".into(), "Glob".into(), "Grep".into()],
        can_delegate_to: vec![],
        max_concurrent_instances: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_roles_count() {
        let roles = builtin_roles();
        assert_eq!(roles.len(), 5);
    }

    #[test]
    fn test_builtin_role_names() {
        let roles = builtin_roles();
        let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"planner"));
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"reviewer"));
        assert!(names.contains(&"executor"));
    }

    #[test]
    fn test_coder_has_write_tools() {
        let roles = builtin_roles();
        let coder = roles.iter().find(|r| r.name == "coder").unwrap();
        assert!(coder.allowed_tools.contains(&"Write".to_string()));
        assert!(coder.allowed_tools.contains(&"Edit".to_string()));
        assert!(coder.allowed_tools.contains(&"Bash".to_string()));
    }

    #[test]
    fn test_reviewer_is_read_only() {
        let roles = builtin_roles();
        let reviewer = roles.iter().find(|r| r.name == "reviewer").unwrap();
        assert!(!reviewer.allowed_tools.contains(&"Write".to_string()));
        assert!(!reviewer.allowed_tools.contains(&"Edit".to_string()));
        assert!(!reviewer.allowed_tools.contains(&"Bash".to_string()));
    }

    #[test]
    fn test_prompt_templates_have_placeholders() {
        let roles = builtin_roles();
        for role in &roles {
            assert!(
                role.system_prompt_template.contains("{{ task.name }}"),
                "role {} missing task.name placeholder",
                role.name
            );
            assert!(
                role.system_prompt_template.contains("{{ subtask.name }}"),
                "role {} missing subtask.name placeholder",
                role.name
            );
        }
    }
}
