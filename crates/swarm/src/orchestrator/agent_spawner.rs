use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::SwarmConfig;
use crate::error::SwarmError;
use crate::types::agent::AgentSession;
use crate::types::plan::SwarmSubtask;

/// Result of a completed agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub session_id: String,
    pub result_text: String,
    pub exit_code: i32,
}

/// Message streamed from the Agent SDK bridge.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AgentMessage {
    /// Agent produced text output.
    #[serde(rename = "text")]
    Text { content: String },
    /// Agent made a tool call.
    #[serde(rename = "tool_use")]
    ToolUse {
        tool: String,
        input: serde_json::Value,
    },
    /// Agent completed.
    #[serde(rename = "result")]
    Result { content: String, session_id: String },
    /// Agent encountered an error.
    #[serde(rename = "error")]
    Error { message: String },
}

/// Spawn an agent session using the Claude Agent SDK (TypeScript bridge).
///
/// The bridge script (`agent-bridge.mjs`) is a thin wrapper around
/// `@anthropic-ai/claude-agent-sdk` that:
/// - Accepts prompt, system prompt, allowed tools, and config via CLI args
/// - Streams NDJSON messages to stdout (text, `tool_use`, result, error)
/// - Uses the user's existing Claude Code authentication (no API keys)
///
/// Falls back to `claude -p` if the Agent SDK bridge is not available.
pub async fn spawn_agent(
    config: &SwarmConfig,
    session: &AgentSession,
    subtask: &SwarmSubtask,
    system_prompt: &str,
    allowed_tools: &[String],
    hooks_binary: &Path,
) -> Result<Child, SwarmError> {
    let workspace = &session.workspace;

    // Generate .claude/settings.json with hooks pointing to our hook binary.
    // Only if the hook binary exists — otherwise agents run without Acteon gating.
    if hooks_binary.exists() {
        setup_workspace_hooks(workspace, hooks_binary, config, &session.id).await?;
    }

    // Agent execution: claude -p is the default (supports multi-turn tool loops).
    // The Agent SDK bridge can be opted in via SWARM_USE_SDK=1 env var, but
    // it currently only supports single-turn tool calls (SDK limitation).
    let bridge = if std::env::var("SWARM_USE_SDK").is_ok() {
        find_agent_bridge()
    } else {
        None
    };

    let tools_arg = allowed_tools.join(",");
    let swarm_env = [
        ("ACTEON_URL", config.acteon.endpoint.as_str()),
        ("ACTEON_AGENT_ROLE", session.role.as_str()),
        ("SWARM_RUN_ID", session.task_id.as_str()),
        ("SWARM_TASK_ID", session.task_id.as_str()),
        ("SWARM_SUBTASK_ID", session.subtask_id.as_str()),
        ("SWARM_AGENT_ID", session.id.as_str()),
    ];

    let child = match bridge {
        Some(BridgeKind::Python(path)) => {
            tracing::info!("using Python Agent SDK bridge");
            let mut cmd = Command::new("python3.10");
            cmd.arg(&path)
                .arg("--prompt")
                .arg(&subtask.prompt)
                .arg("--system-prompt")
                .arg(system_prompt)
                .arg("--cwd")
                .arg(workspace)
                .arg("--model")
                .arg("sonnet");
            // Don't pass --allowed-tools to the SDK bridge: let the agent
            // use all tools freely. Role restrictions are advisory; Acteon
            // hooks enforce safety. Restricting tools causes ToolSearch
            // overhead where the agent wastes turns discovering deferred tools.
            for (k, v) in &swarm_env {
                cmd.env(k, v);
            }
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(workspace)
                .spawn()
                .map_err(|e| {
                    SwarmError::AgentSpawn(format!("failed to spawn Python bridge: {e}"))
                })?
        }
        Some(BridgeKind::Node(path)) => {
            tracing::info!("using Node.js Agent SDK bridge");
            let mut cmd = Command::new("node");
            cmd.arg(&path)
                .arg("--prompt")
                .arg(&subtask.prompt)
                .arg("--system-prompt")
                .arg(system_prompt)
                .arg("--allowed-tools")
                .arg(&tools_arg)
                .arg("--cwd")
                .arg(workspace)
                .current_dir(workspace);
            for (k, v) in &swarm_env {
                cmd.env(k, v);
            }
            cmd.env_optional("ACTEON_AGENT_KEY", config.acteon.api_key.as_deref())
                .env_optional("TESSERAI_URL", Some(config.tesserai.endpoint.as_str()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn Node bridge: {e}")))?
        }
        None => {
            tracing::warn!("Agent SDK bridge not found, falling back to `claude -p`");
            let full_prompt = format!("{system_prompt}\n\n## Task\n{}", subtask.prompt);
            let mut cmd = Command::new("claude");
            cmd.arg("-p")
                .arg(&full_prompt)
                .arg("--model")
                .arg("sonnet")
                .arg("--allowedTools")
                .arg(&tools_arg)
                .arg("--output-format")
                .arg("json")
                .current_dir(workspace);
            for (k, v) in &swarm_env {
                cmd.env(k, v);
            }
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn claude: {e}")))?
        }
    };

    Ok(child)
}

/// Read NDJSON messages from a running agent's stdout.
pub async fn read_agent_messages(child: &mut Child) -> Result<Vec<AgentMessage>, SwarmError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SwarmError::AgentSpawn("no stdout handle".into()))?;

    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    let mut messages = Vec::new();

    while let Some(line) = lines.next_line().await.map_err(|e: std::io::Error| {
        SwarmError::AgentSpawn(format!("failed to read agent output: {e}"))
    })? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AgentMessage>(&line) {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                tracing::debug!("skipping non-JSON agent output: {e}");
            }
        }
    }

    Ok(messages)
}

/// Wait for an agent process to complete and collect its result.
pub async fn wait_for_agent(
    child: Child,
    session_id: &str,
    timeout_seconds: u64,
) -> Result<AgentResult, SwarmError> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_seconds),
        child.wait_with_output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let mut exit_code = output.status.code().unwrap_or(-1);
            let result_text = String::from_utf8_lossy(&output.stdout).to_string();

            // claude -p --output-format json always exits 0; check is_error in JSON.
            if exit_code == 0
                && serde_json::from_str::<serde_json::Value>(&result_text)
                    .ok()
                    .and_then(|json| json.get("is_error").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false)
            {
                exit_code = 1;
            }

            Ok(AgentResult {
                session_id: session_id.into(),
                result_text,
                exit_code,
            })
        }
        Ok(Err(e)) => Err(SwarmError::AgentSpawn(format!("agent process error: {e}"))),
        Err(_) => {
            // Timeout: child is consumed by wait_with_output; just report.
            Err(SwarmError::AgentTimeout {
                agent_id: session_id.into(),
                timeout_seconds,
            })
        }
    }
}

// ── Workspace setup ──────────────────────────────────────────────────────────

/// Generate .claude/settings.json in the agent workspace with hooks.
async fn setup_workspace_hooks(
    workspace: &Path,
    hooks_binary: &Path,
    config: &SwarmConfig,
    agent_id: &str,
) -> Result<(), SwarmError> {
    let claude_dir = workspace.join(".claude");
    tokio::fs::create_dir_all(&claude_dir)
        .await
        .map_err(|e| SwarmError::WorkspaceSetup {
            path: claude_dir.clone(),
            reason: format!("failed to create .claude directory: {e}"),
        })?;

    let hook_bin = hooks_binary.display();
    let acteon_url = &config.acteon.endpoint;
    let tesserai_url = &config.tesserai.endpoint;

    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash|Write|Edit|WebFetch|WebSearch|Task",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} gate --acteon-url {acteon_url} --agent-id {agent_id}"),
                    "timeout": 15
                }]
            }],
            "PostToolUse": [{
                "matcher": "Bash|Write|Edit",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} record --tesserai-url {tesserai_url} --agent-id {agent_id}"),
                    "timeout": 10,
                    "async": true
                }]
            }],
            "Stop": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} complete --acteon-url {acteon_url} --tesserai-url {tesserai_url} --agent-id {agent_id}"),
                    "timeout": 15,
                    "async": true
                }]
            }]
        }
    });

    let settings_path = claude_dir.join("settings.json");
    tokio::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)
        .await
        .map_err(|e| SwarmError::WorkspaceSetup {
            path: settings_path,
            reason: format!("failed to write settings.json: {e}"),
        })?;

    Ok(())
}

/// Which Agent SDK bridge was found.
enum BridgeKind {
    Python(PathBuf),
    Node(PathBuf),
}

/// Find the Agent SDK bridge script.
///
/// Search order: Python (`agent-bridge.py`) > Node.js (`agent-bridge.mjs`).
/// Looks next to the binary, then in `bridge/` relative to CWD.
fn find_agent_bridge() -> Option<BridgeKind> {
    let candidates = [
        (
            "agent-bridge.py",
            BridgeKind::Python as fn(PathBuf) -> BridgeKind,
        ),
        (
            "agent-bridge.mjs",
            BridgeKind::Node as fn(PathBuf) -> BridgeKind,
        ),
    ];

    for (filename, make) in &candidates {
        // Check next to the current binary.
        if let Ok(exe) = std::env::current_exe() {
            let path = exe.parent().unwrap_or(Path::new(".")).join(filename);
            if path.exists() {
                return Some(make(path));
            }
        }

        // Check relative to CWD.
        let local = PathBuf::from(format!("bridge/{filename}"));
        if local.exists() {
            return Some(make(local));
        }
    }

    None
}

/// Extension trait to conditionally set environment variables.
trait CommandEnvExt {
    fn env_optional(&mut self, key: &str, value: Option<&str>) -> &mut Self;
}

impl CommandEnvExt for Command {
    fn env_optional(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        if let Some(v) = value {
            self.env(key, v);
        }
        self
    }
}
