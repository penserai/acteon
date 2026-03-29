use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::types::plan::SwarmPlan;

/// Build the system prompt for plan gathering.
///
/// This prompt instructs the AI to interactively gather requirements
/// and produce a structured [`SwarmPlan`] as JSON output.
pub fn build_gathering_prompt(config: &SwarmConfig) -> String {
    let max_agents = config.defaults.max_agents;
    let max_duration = config.defaults.max_duration_minutes;
    let working_dir = config
        .defaults
        .working_directory
        .as_ref()
        .map_or_else(|| ".".to_string(), |p| p.display().to_string());

    let engine_tools = match config.defaults.engine {
        crate::config::AgentEngine::Claude => {
            r"
- **planner**: Read-only analysis (tools: Read, Glob, Grep)
- **coder**: Code modification (tools: Read, Write, Edit, Bash, Glob, Grep)
- **researcher**: Web research (tools: Read, Glob, Grep, WebFetch, WebSearch)
"
        }
        crate::config::AgentEngine::Gemini => {
            r"
- **planner**: Read-only analysis (tools: read_file, glob, grep_search)
- **coder**: Code modification (tools: read_file, write_file, replace, run_shell_command, glob, grep_search)
- **researcher**: Web research (tools: read_file, glob, grep_search, web_fetch, google_web_search)
"
        }
    };

    format!(
        r#"You are a swarm planning assistant. Your job is to gather requirements from the user and produce a structured execution plan for a multi-agent swarm.

## Constraints
- Maximum {max_agents} concurrent agents
- Maximum {max_duration} minutes total execution time
- Working directory: {working_dir}

## Available Agent Roles
{engine_tools}
- **reviewer**: Code review (read-only tools)
- **executor**: Command execution and testing (execution tools)

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

/// Gather a plan by invoking the AI engine with the gathering prompt.
pub async fn gather_plan(config: &SwarmConfig, user_prompt: &str) -> Result<SwarmPlan, SwarmError> {
    let system_prompt = build_gathering_prompt(config);
    let schema = plan_json_schema();
    let schema_str = serde_json::to_string(&schema)?;
    let engine = config.defaults.engine;

    let full_prompt = if matches!(engine, crate::config::AgentEngine::Gemini) {
        format!(
            "{system_prompt}\n\n## Output Schema (JSON)\n{schema_str}\n\n## User Request\n{user_prompt}"
        )
    } else {
        format!("{system_prompt}\n\n## User Request\n{user_prompt}")
    };

    let cmd_name = match engine {
        crate::config::AgentEngine::Claude => "claude",
        crate::config::AgentEngine::Gemini => "gemini",
    };

    let mut cmd = tokio::process::Command::new(cmd_name);
    cmd.arg("-p")
        .arg(&full_prompt)
        .arg("--output-format")
        .arg("json");

    if matches!(engine, crate::config::AgentEngine::Claude) {
        cmd.arg("--json-schema").arg(&schema_str);
    }

    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .map_err(|e| SwarmError::PlanGathering(format!("failed to spawn {cmd_name}: {e}")))?;

    if !output.status.success() {
        return Err(SwarmError::PlanGathering(format!(
            "{cmd_name} exited with status {}",
            output.status
        )));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);

    // Try parsing as JSON envelope first (Claude: {"result":"...", "structured_output":...})
    // then fall back to raw text (Gemini: plain text or markdown with embedded JSON).
    let plan: SwarmPlan = if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&stdout_str) {
        // JSON envelope — try structured_output, result, or response fields.
        let plan_value = raw
            .get("structured_output")
            .or_else(|| raw.get("result"))
            .or_else(|| raw.get("response"))
            .ok_or_else(|| {
                SwarmError::PlanGathering(format!(
                    "{cmd_name} output missing structured_output, result and response"
                ))
            })?;

        if let Some(s) = plan_value.as_str() {
            let stripped = strip_markdown(s);
            serde_json::from_str(&stripped).map_err(|e| {
                SwarmError::PlanGathering(format!(
                    "failed to parse plan from result string: {e}\nString: {stripped}"
                ))
            })?
        } else {
            serde_json::from_value(plan_value.clone()).map_err(|e| {
                SwarmError::PlanGathering(format!(
                    "failed to parse plan from structured output: {e}"
                ))
            })?
        }
    } else {
        // Raw text output (Gemini) — strip markdown fences and parse JSON.
        let stripped = strip_markdown(&stdout_str);
        serde_json::from_str(&stripped).map_err(|e| {
            SwarmError::PlanGathering(format!(
                "failed to parse plan from raw output: {e}\nOutput: {}",
                &stripped[..stripped.len().min(500)]
            ))
        })?
    };

    Ok(plan)
}

/// Strip markdown code fences from text that may contain JSON.
///
/// Handles cases where there's prose before/after the code fence
/// (common with Gemini output like "Here's the plan:\n```json\n{...}\n```").
fn strip_markdown(s: &str) -> String {
    let s = s.trim();

    // Find ```json or ``` fence within the text.
    if let Some(start) = s.find("```json") {
        let json_start = start + 7;
        if let Some(end) = s[json_start..].find("```") {
            return s[json_start..json_start + end].trim().to_string();
        }
    }
    if let Some(start) = s.find("```") {
        let content_start = start + 3;
        if let Some(end) = s[content_start..].find("```") {
            return s[content_start..content_start + end].trim().to_string();
        }
    }

    s.to_string()
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
