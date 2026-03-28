use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::types::plan::SwarmPlan;

/// Build the system prompt for plan gathering.
///
/// This prompt instructs Claude to interactively gather requirements
/// and produce a structured [`SwarmPlan`] as JSON output.
pub fn build_gathering_prompt(config: &SwarmConfig) -> String {
    let max_agents = config.defaults.max_agents;
    let max_duration = config.defaults.max_duration_minutes;
    let working_dir = config
        .defaults
        .working_directory
        .as_ref()
        .map_or_else(|| ".".to_string(), |p| p.display().to_string());

    format!(
        r#"You are a swarm planning assistant. Your job is to gather requirements from the user and produce a structured execution plan for a multi-agent swarm.

## Constraints
- Maximum {max_agents} concurrent agents
- Maximum {max_duration} minutes total execution time
- Working directory: {working_dir}

## Available Agent Roles
- **planner**: Read-only analysis and decomposition (tools: Read, Glob, Grep)
- **coder**: Code writing and modification (tools: Read, Write, Edit, Bash, Glob, Grep)
- **researcher**: Documentation and web research (tools: Read, Glob, Grep, WebFetch, WebSearch)
- **reviewer**: Code review and issue identification (tools: Read, Glob, Grep)
- **executor**: Command execution and testing (tools: Bash, Read, Glob, Grep)

## Your Process
1. Understand the user's objective
2. Ask clarifying questions about scope, constraints, and success criteria
3. Decompose into tasks with dependencies
4. Assign each task to an appropriate agent role
5. Break each task into concrete subtasks with agent prompts
6. Estimate the total action count

## Output Format
Output a single JSON object matching the SwarmPlan schema. Each task should have:
- A unique ID (e.g., "task-1", "task-2")
- Subtasks with unique IDs (e.g., "task-1-sub-1")
- Clear dependency declarations
- Concrete agent prompts for each subtask"#
    )
}

/// JSON schema for structured output from `claude -p --json-schema`.
pub fn plan_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["id", "objective", "scope", "success_criteria", "tasks", "agent_roles", "estimated_actions"],
        "properties": {
            "id": { "type": "string" },
            "objective": { "type": "string" },
            "scope": {
                "type": "object",
                "required": ["working_directory"],
                "properties": {
                    "working_directory": { "type": "string" },
                    "allowed_paths": { "type": "array", "items": { "type": "string" } },
                    "forbidden_patterns": { "type": "array", "items": { "type": "string" } },
                    "max_agents": { "type": "integer" },
                    "max_duration_minutes": { "type": "integer" },
                    "require_approval_for": { "type": "array", "items": { "type": "string" } }
                }
            },
            "success_criteria": { "type": "array", "items": { "type": "string" } },
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "name", "description", "assigned_role", "subtasks"],
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "assigned_role": { "type": "string" },
                        "subtasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["id", "name", "description", "prompt"],
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "description": { "type": "string" },
                                    "prompt": { "type": "string" },
                                    "allowed_tools": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    },
                                    "timeout_seconds": { "type": "integer" }
                                }
                            }
                        },
                        "depends_on": { "type": "array", "items": { "type": "string" } },
                        "priority": { "type": "integer" }
                    }
                }
            },
            "agent_roles": { "type": "array", "items": { "type": "string" } },
            "estimated_actions": { "type": "integer" }
        }
    })
}

/// Gather a plan by invoking `claude -p` with the gathering prompt.
///
/// This spawns a Claude Code subprocess in print mode with structured
/// JSON output. The user's existing Claude Code authentication is used
/// (no API keys required).
pub async fn gather_plan(config: &SwarmConfig, user_prompt: &str) -> Result<SwarmPlan, SwarmError> {
    let system_prompt = build_gathering_prompt(config);
    let schema = plan_json_schema();
    let schema_str = serde_json::to_string(&schema)?;

    let full_prompt = format!("{system_prompt}\n\n## User Request\n{user_prompt}");

    let output = tokio::process::Command::new("claude")
        .arg("-p")
        .arg(&full_prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--json-schema")
        .arg(&schema_str)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .map_err(|e| SwarmError::PlanGathering(format!("failed to spawn claude: {e}")))?;

    if !output.status.success() {
        return Err(SwarmError::PlanGathering(format!(
            "claude exited with status {}",
            output.status
        )));
    }

    // Claude --output-format json wraps result in {"result": "...", "session_id": "..."}
    // The structured_output field contains our plan when --json-schema is used.
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        SwarmError::PlanGathering(format!(
            "failed to parse claude output: {e}\nstdout: {}",
            String::from_utf8_lossy(&output.stdout)
        ))
    })?;

    // Try structured_output first (when --json-schema is used), then result.
    let plan_value = raw
        .get("structured_output")
        .or_else(|| raw.get("result"))
        .ok_or_else(|| {
            SwarmError::PlanGathering("claude output missing structured_output and result".into())
        })?;

    // If result is a string, it may contain JSON that needs re-parsing.
    let plan: SwarmPlan = if let Some(s) = plan_value.as_str() {
        serde_json::from_str(s).map_err(|e| {
            SwarmError::PlanGathering(format!("failed to parse plan from result string: {e}"))
        })?
    } else {
        serde_json::from_value(plan_value.clone()).map_err(|e| {
            SwarmError::PlanGathering(format!("failed to parse plan from structured output: {e}"))
        })?
    };

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gathering_prompt_contains_roles() {
        let config = SwarmConfig::minimal();
        let prompt = build_gathering_prompt(&config);
        assert!(prompt.contains("planner"));
        assert!(prompt.contains("coder"));
        assert!(prompt.contains("researcher"));
        assert!(prompt.contains("reviewer"));
        assert!(prompt.contains("executor"));
    }

    #[test]
    fn test_plan_json_schema_structure() {
        let schema = plan_json_schema();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "objective"));
        assert!(required.iter().any(|v| v == "tasks"));
    }
}
