use md5::{Digest, Md5};
use serde::Deserialize;

use crate::error::SwarmError;

/// Tool call input from Claude Code `PreToolUse` hook (JSON via stdin).
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub session_id: String,
}

/// Map agent tool names to Acteon action types.
///
/// Supports both Claude Code and Gemini CLI tool naming conventions.
pub fn map_tool_to_action_type(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        // Claude Code: Bash, Gemini CLI: run_shell_command
        "Bash" | "run_shell_command" => Some("execute_command"),
        // Claude Code: Write/Edit, Gemini CLI: write_file/replace
        "Write" | "Edit" | "write_file" | "replace" => Some("write_file"),
        // Claude Code: WebFetch/WebSearch, Gemini CLI: web_fetch/google_web_search
        "WebFetch" | "WebSearch" | "web_fetch" | "google_web_search" => Some("web_access"),
        // Claude Code: Task, Gemini CLI: generalist
        "Task" | "generalist" => Some("spawn_agent"),

        // Read-only tools pass through without gating.
        _ => None,
    }
}

/// Build a deduplication key for cross-agent coordination.
///
/// For `write_file` actions, uses the file path hash so writes to the
/// same file from different agents are deduplicated.
/// For other actions, uses session-scoped keys.
pub fn build_dedup_key(
    action_type: &str,
    session_id: &str,
    tool_input: &serde_json::Value,
) -> String {
    if action_type == "write_file" {
        let file_path = tool_input
            .get("file_path")
            .or_else(|| tool_input.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !file_path.is_empty() {
            return format!("write-{}", md5_hex(file_path));
        }
    }

    let input_str = serde_json::to_string(tool_input).unwrap_or_default();
    format!("{session_id}-{action_type}-{}", md5_hex(&input_str))
}

/// Dispatch a tool call to Acteon for policy enforcement.
///
/// Returns `Ok(true)` if the action is allowed (exit 0),
/// `Ok(false)` if blocked (exit 2).
pub async fn dispatch_to_acteon(
    acteon_url: &str,
    api_key: Option<&str>,
    namespace: &str,
    tenant: &str,
    agent_role: &str,
    agent_id: &str,
    input: &HookInput,
) -> Result<bool, SwarmError> {
    let Some(action_type) = map_tool_to_action_type(&input.tool_name) else {
        return Ok(true); // Read-only tool, allow.
    };

    let dedup_key = build_dedup_key(action_type, &input.session_id, &input.tool_input);
    let action_id = uuid::Uuid::new_v4().to_string();

    let engine_provider = if input.tool_name.contains('_') || input.tool_name == "generalist" {
        "gemini-cli"
    } else {
        "claude-code"
    };

    let body = serde_json::json!({
        "id": action_id,
        "namespace": namespace,
        "tenant": tenant,
        "provider": engine_provider,
        "action_type": action_type,
        "payload": input.tool_input,
        "metadata": {
            "tool_name": input.tool_name,
            "session_id": input.session_id,
            "agent_role": agent_role,
            "agent_id": agent_id,
        },
        "created_at": chrono::Utc::now().to_rfc3339(),
        "dedup_key": dedup_key,
    });

    let client = reqwest::Client::new();
    let url = format!("{acteon_url}/v1/dispatch");
    let mut req = client.post(&url).json(&body);

    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Fail closed: if Acteon is unreachable, block.
            eprintln!("Acteon gateway unreachable at {acteon_url}: {e}");
            return Ok(false);
        }
    };

    if !resp.status().is_success() {
        eprintln!("Acteon returned HTTP {} -- blocking action", resp.status());
        return Ok(false);
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| SwarmError::Hook(format!("failed to parse Acteon response: {e}")))?;

    // Response is a Rust enum: {"Executed":{...}}, {"Suppressed":{...}}, etc.
    let outcome = body
        .as_object()
        .and_then(|obj| obj.keys().next())
        .map_or("unknown", String::as_str);

    match outcome {
        "Executed" | "Deduplicated" => Ok(true),
        "PendingApproval" => {
            let approval_id = body["PendingApproval"]["approval_id"]
                .as_str()
                .unwrap_or("unknown");
            eprintln!("Action held for human approval (ID: {approval_id})");
            Ok(false)
        }
        "Suppressed" => {
            let rule = body["Suppressed"]["rule"]
                .as_str()
                .unwrap_or("unknown rule");
            eprintln!("BLOCKED by Acteon rule '{rule}'");
            Ok(false)
        }
        "Throttled" => {
            let retry = body["Throttled"]["retry_after"]["secs"]
                .as_u64()
                .unwrap_or(0);
            eprintln!("Rate limited -- retry after {retry}s");
            Ok(false)
        }
        "Rerouted" => {
            let target = body["Rerouted"]["target_provider"]
                .as_str()
                .unwrap_or("unknown");
            eprintln!("Action rerouted to '{target}'");
            Ok(true)
        }
        "QuotaExceeded" => {
            eprintln!("Tenant quota exceeded -- action blocked");
            Ok(false)
        }
        _ => {
            eprintln!("Unexpected Acteon outcome: {outcome} -- blocking for safety");
            Ok(false)
        }
    }
}

fn md5_hex(input: &str) -> String {
    let result = Md5::digest(input.as_bytes());
    format!("{result:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_tool_to_action_type() {
        assert_eq!(map_tool_to_action_type("Bash"), Some("execute_command"));
        assert_eq!(map_tool_to_action_type("Write"), Some("write_file"));
        assert_eq!(map_tool_to_action_type("Edit"), Some("write_file"));
        assert_eq!(map_tool_to_action_type("WebFetch"), Some("web_access"));
        assert_eq!(map_tool_to_action_type("Read"), None);
        assert_eq!(map_tool_to_action_type("Glob"), None);
        assert_eq!(map_tool_to_action_type("Grep"), None);
    }

    #[test]
    fn test_dedup_key_write_file() {
        let input = serde_json::json!({"file_path": "/tmp/foo.rs"});
        let key = build_dedup_key("write_file", "session-1", &input);
        assert!(key.starts_with("write-"));
        // Same file path should produce same key regardless of session.
        let key2 = build_dedup_key("write_file", "session-2", &input);
        assert_eq!(key, key2);
    }

    #[test]
    fn test_dedup_key_command() {
        let input = serde_json::json!({"command": "cargo test"});
        let key = build_dedup_key("execute_command", "session-1", &input);
        assert!(key.starts_with("session-1-execute_command-"));
    }

    #[test]
    fn test_hook_input_parse() {
        let json = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"session_id":"abc"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_name, "Bash");
        assert_eq!(input.session_id, "abc");
    }
}
