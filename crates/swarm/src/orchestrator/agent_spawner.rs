use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
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
    run_id: &str,
) -> Result<Child, SwarmError> {
    let workspace = &session.workspace;
    let engine = config.defaults.engine;

    // Generate settings with hooks pointing to our hook binary.
    if !hooks_binary.is_file() {
        return Err(SwarmError::AgentSpawn(format!(
            "required policy hook binary is missing: {}",
            hooks_binary.display()
        )));
    }
    setup_workspace_hooks(workspace, hooks_binary, config, &session.id).await?;

    // Per-run tenant isolation: the PreToolUse gate reads ACTEON_NAMESPACE /
    // ACTEON_TENANT from the inherited environment, so every gated action for
    // this run is dispatched (and audited) under `swarm-{run_id}`. This is what
    // makes `cmd_status` and `cmd_cancel` able to target a single run — without
    // it the gate falls back to the shared `swarm-default` tenant.
    let tenant = format!("swarm-{run_id}");
    let swarm_env = [
        ("ACTEON_URL", config.acteon.endpoint.as_str()),
        ("ACTEON_NAMESPACE", config.acteon.namespace.as_str()),
        ("ACTEON_TENANT", tenant.as_str()),
        ("ACTEON_AGENT_ROLE", session.role.as_str()),
        ("SWARM_RUN_ID", run_id),
        ("SWARM_TASK_ID", session.task_id.as_str()),
        ("SWARM_SUBTASK_ID", session.subtask_id.as_str()),
        ("SWARM_AGENT_ID", session.id.as_str()),
    ];

    let child = match engine {
        crate::config::AgentEngine::Claude => spawn_claude_agent(
            config,
            subtask,
            system_prompt,
            allowed_tools,
            workspace,
            &swarm_env,
        )?,
        crate::config::AgentEngine::Gemini => spawn_gemini_agent(
            subtask,
            system_prompt,
            workspace,
            &swarm_env,
            config.acteon.api_key.as_deref(),
        )?,
    };

    Ok(child)
}

/// Spawn a Claude agent, preferring the Agent SDK bridge when available.
fn spawn_claude_agent(
    config: &SwarmConfig,
    subtask: &SwarmSubtask,
    system_prompt: &str,
    allowed_tools: &[String],
    workspace: &Path,
    swarm_env: &[(&str, &str)],
) -> Result<Child, SwarmError> {
    let bridge = if std::env::var("SWARM_USE_SDK").is_ok() {
        find_agent_bridge()
    } else {
        None
    };

    match bridge {
        Some(BridgeKind::Python(path)) => {
            tracing::info!("using Python Agent SDK bridge");
            let mut cmd = Command::new("python3.10");
            super::process::configure(&mut cmd);
            cmd.arg(&path)
                .arg("--prompt")
                .arg(&subtask.prompt)
                .arg("--system-prompt")
                .arg(system_prompt)
                .arg("--cwd")
                .arg(workspace)
                .arg("--model")
                .arg("sonnet");
            for (k, v) in swarm_env {
                cmd.env(k, v);
            }
            cmd.env_optional("ACTEON_AGENT_KEY", config.acteon.api_key.as_deref())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .current_dir(workspace)
                .spawn()
                .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn Python bridge: {e}")))
        }
        Some(BridgeKind::Node(path)) => {
            tracing::info!("using Node.js Agent SDK bridge");
            let mut cmd = Command::new("node");
            super::process::configure(&mut cmd);
            cmd.arg(&path)
                .arg("--prompt")
                .arg(&subtask.prompt)
                .arg("--system-prompt")
                .arg(system_prompt)
                .arg("--allowed-tools")
                .arg(allowed_tools.join(","))
                .arg("--cwd")
                .arg(workspace)
                .current_dir(workspace);
            for (k, v) in swarm_env {
                cmd.env(k, v);
            }
            cmd.env_optional("ACTEON_AGENT_KEY", config.acteon.api_key.as_deref())
                .env_optional("TESSERAI_URL", Some(config.tesserai.endpoint.as_str()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn Node bridge: {e}")))
        }
        None => {
            tracing::warn!("Agent SDK bridge not found, falling back to `claude -p`");
            let full_prompt = format!("{system_prompt}\n\n## Task\n{}", subtask.prompt);
            let mut cmd = Command::new("claude");
            super::process::configure(&mut cmd);
            cmd.arg("-p")
                .arg(&full_prompt)
                .arg("--model")
                .arg("sonnet")
                .arg("--allowedTools")
                .arg(allowed_tools.join(","))
                .arg("--output-format")
                .arg("json")
                .current_dir(workspace);
            for (k, v) in swarm_env {
                cmd.env(k, v);
            }
            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn claude: {e}")))
        }
    }
}

/// Spawn a Gemini CLI agent process.
fn spawn_gemini_agent(
    subtask: &SwarmSubtask,
    system_prompt: &str,
    workspace: &Path,
    swarm_env: &[(&str, &str)],
    api_key: Option<&str>,
) -> Result<Child, SwarmError> {
    tracing::info!("spawning gemini agent");
    let full_prompt = format!("{system_prompt}\n\n## Task\n{}", subtask.prompt);
    let mut cmd = Command::new("gemini");
    super::process::configure(&mut cmd);
    cmd.arg("-p")
        .arg(&full_prompt)
        .arg("--yolo")
        .arg("--output-format")
        .arg("json")
        .current_dir(workspace);
    for (k, v) in swarm_env {
        cmd.env(k, v);
    }
    cmd.env_optional("ACTEON_AGENT_KEY", api_key)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SwarmError::AgentSpawn(format!("failed to spawn gemini: {e}")))
}

/// Read NDJSON messages from a running agent's stdout.
pub async fn read_agent_messages(child: &mut Child) -> Result<Vec<AgentMessage>, SwarmError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SwarmError::AgentSpawn("no stdout handle".into()))?;

    let mut bytes = Vec::new();
    stdout
        .take((super::process::MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > super::process::MAX_OUTPUT_BYTES {
        return Err(SwarmError::AgentSpawn(
            "agent output exceeded 1 MiB limit".into(),
        ));
    }
    let output = String::from_utf8(bytes)
        .map_err(|e| SwarmError::AgentSpawn(format!("invalid agent output: {e}")))?;
    let mut messages = Vec::new();

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AgentMessage>(line) {
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
    let result = super::process::wait(child, std::time::Duration::from_secs(timeout_seconds)).await;

    match result {
        Ok(output) => {
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
        Err(e) if e.kind() != std::io::ErrorKind::TimedOut => {
            Err(SwarmError::AgentSpawn(format!("agent process error: {e}")))
        }
        Err(_) => {
            // Managed process group has been terminated and the child reaped.
            Err(SwarmError::AgentTimeout {
                agent_id: session_id.into(),
                timeout_seconds,
            })
        }
    }
}

// ── Workspace setup ──────────────────────────────────────────────────────────

/// Generate hook settings in the agent workspace.
async fn setup_workspace_hooks(
    workspace: &Path,
    hooks_binary: &Path,
    config: &SwarmConfig,
    agent_id: &str,
) -> Result<(), SwarmError> {
    let engine = config.defaults.engine;
    let hook_bin = hooks_binary.display().to_string();
    let acteon_url = &config.acteon.endpoint;
    let tesserai_url = &config.tesserai.endpoint;

    match engine {
        crate::config::AgentEngine::Claude => {
            setup_claude_hooks(workspace, &hook_bin, acteon_url, tesserai_url, agent_id).await
        }
        crate::config::AgentEngine::Gemini => {
            setup_gemini_hooks(workspace, &hook_bin, acteon_url, tesserai_url, agent_id).await
        }
    }
}

/// Write Claude Code hook settings into `<workspace>/.claude/settings.json`.
async fn setup_claude_hooks(
    workspace: &Path,
    hook_bin: &str,
    acteon_url: &str,
    tesserai_url: &str,
    agent_id: &str,
) -> Result<(), SwarmError> {
    let (hook_bin, acteon_url, tesserai_url, agent_id) = (
        shell_word(hook_bin),
        shell_word(acteon_url),
        shell_word(tesserai_url),
        shell_word(agent_id),
    );
    let claude_dir = workspace.join(".claude");
    tokio::fs::create_dir_all(&claude_dir)
        .await
        .map_err(|e| SwarmError::WorkspaceSetup {
            path: claude_dir.clone(),
            reason: format!("failed to create .claude directory: {e}"),
        })?;

    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "",
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
    merge_hook_settings(&settings_path, &settings, &hook_bin).await
}

/// Write Gemini CLI hook settings into `<workspace>/.gemini/settings.json`.
async fn setup_gemini_hooks(
    workspace: &Path,
    hook_bin: &str,
    acteon_url: &str,
    tesserai_url: &str,
    agent_id: &str,
) -> Result<(), SwarmError> {
    let (hook_bin, acteon_url, tesserai_url, agent_id) = (
        shell_word(hook_bin),
        shell_word(acteon_url),
        shell_word(tesserai_url),
        shell_word(agent_id),
    );
    let gemini_dir = workspace.join(".gemini");
    tokio::fs::create_dir_all(&gemini_dir)
        .await
        .map_err(|e| SwarmError::WorkspaceSetup {
            path: gemini_dir.clone(),
            reason: format!("failed to create .gemini directory: {e}"),
        })?;

    let settings = serde_json::json!({
        "hooks": {
            "BeforeTool": [{
                "matcher": ".*",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} gate --acteon-url {acteon_url} --agent-id {agent_id}"),
                    "timeout": 15
                }]
            }],
            "AfterTool": [{
                "matcher": "run_shell_command|write_file|replace",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} record --tesserai-url {tesserai_url} --agent-id {agent_id}"),
                    "timeout": 10,
                    "async": true
                }]
            }],
            "SessionEnd": [{
                "matcher": ".*",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hook_bin} complete --acteon-url {acteon_url} --tesserai-url {tesserai_url} --agent-id {agent_id}"),
                    "timeout": 15,
                    "async": true
                }]
            }]
        }
    });

    let settings_path = gemini_dir.join("settings.json");
    merge_hook_settings(&settings_path, &settings, &hook_bin).await
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Preserve operator settings and unrelated hooks; replace only this binary's entries.
async fn merge_hook_settings(
    path: &Path,
    generated: &serde_json::Value,
    binary: &str,
) -> Result<(), SwarmError> {
    let mut settings: serde_json::Value = match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    if settings
        .get("disableAllHooks")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Err(SwarmError::Hook(
            "workspace disables required policy hooks".into(),
        ));
    }
    let object = settings
        .as_object_mut()
        .ok_or_else(|| SwarmError::Hook("hook settings must be an object".into()))?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| SwarmError::Hook("hooks must be an object".into()))?;
    for (event, entries) in generated["hooks"]
        .as_object()
        .expect("generated hooks object")
    {
        let existing = hooks
            .entry(event.clone())
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| SwarmError::Hook("hook event must be an array".into()))?;
        existing.retain(|entry| {
            !entry["hooks"].as_array().is_some_and(|commands| {
                !commands.is_empty()
                    && commands.iter().all(|command| {
                        command["command"].as_str().is_some_and(|command| {
                            ["gate", "record", "complete"]
                                .iter()
                                .any(|mode| command.starts_with(&format!("{binary} {mode} ")))
                        })
                    })
            })
        });
        existing.extend(
            entries
                .as_array()
                .expect("generated hook array")
                .iter()
                .cloned(),
        );
    }
    // A complete file replaces the old one only after serialization and writing succeed.
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    temporary.write_all(&serde_json::to_vec_pretty(&settings)?)?;
    temporary
        .persist(path)
        .map_err(|error| SwarmError::Io(error.error))?;
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

#[cfg(test)]
mod hook_tests {
    use super::*;

    #[tokio::test]
    async fn installing_hooks_preserves_settings_and_gates_new_tools() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".claude")).unwrap();
        let path = directory.path().join(".claude/settings.json");
        std::fs::write(&path, r#"{"model":"operator-choice","hooks":{"PreToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"user-lint"}]}]}}"#).unwrap();
        for id in ["one", "two"] {
            setup_claude_hooks(
                directory.path(),
                "/a path/hook",
                "http://localhost",
                "http://localhost",
                id,
            )
            .await
            .unwrap();
        }
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(settings["model"], "operator-choice");
        let hooks = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0]["hooks"][0]["command"], "user-lint");
        assert_eq!(hooks[1]["matcher"], "");
        assert!(
            hooks[1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .starts_with("'/a path/hook' gate ")
        );
        setup_gemini_hooks(directory.path(), "hook", "url", "url", "id")
            .await
            .unwrap();
        let settings: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.path().join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(settings["hooks"]["BeforeTool"][0]["matcher"], ".*");
    }
}
