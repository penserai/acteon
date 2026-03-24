//! `acteon-swarm-hook` — Lightweight binary for Claude Code hook integration.
//!
//! Three subcommands:
//! - `gate`     — `PreToolUse`: dispatch to Acteon, exit 0 (allow) or 2 (block)
//! - `record`   — `PostToolUse`: store episodic memory in `TesseraiDB`
//! - `complete` — Stop: mark agent session as complete

use std::io::Read;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "acteon-swarm-hook",
    version,
    about = "Claude Code hook handler for acteon-swarm"
)]
struct Cli {
    #[command(subcommand)]
    command: HookCommand,
}

#[derive(Subcommand)]
enum HookCommand {
    /// `PreToolUse`: gate tool calls through Acteon.
    Gate {
        /// Acteon gateway URL.
        #[arg(long, env = "ACTEON_URL", default_value = "http://localhost:8080")]
        acteon_url: String,
        /// Agent session ID.
        #[arg(long, env = "SWARM_AGENT_ID")]
        agent_id: String,
        /// Agent role.
        #[arg(long, env = "ACTEON_AGENT_ROLE", default_value = "coding")]
        agent_role: Option<String>,
        /// API key.
        #[arg(long, env = "ACTEON_AGENT_KEY")]
        api_key: Option<String>,
        /// Namespace.
        #[arg(long, env = "ACTEON_NAMESPACE", default_value = "swarm")]
        namespace: Option<String>,
        /// Tenant.
        #[arg(long, env = "ACTEON_TENANT")]
        tenant: Option<String>,
    },
    /// `PostToolUse`: record action in `TesseraiDB`.
    Record {
        /// `TesseraiDB` URL.
        #[arg(long, env = "TESSERAI_URL", default_value = "http://localhost:8081")]
        tesserai_url: String,
        /// Agent session ID.
        #[arg(long, env = "SWARM_AGENT_ID")]
        agent_id: String,
    },
    /// Stop: mark session complete in `TesseraiDB`.
    Complete {
        /// Acteon gateway URL.
        #[arg(long, env = "ACTEON_URL", default_value = "http://localhost:8080")]
        acteon_url: String,
        /// `TesseraiDB` URL.
        #[arg(long, env = "TESSERAI_URL", default_value = "http://localhost:8081")]
        tesserai_url: String,
        /// Agent session ID.
        #[arg(long, env = "SWARM_AGENT_ID")]
        agent_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Read hook input from stdin.
    let mut input_str = String::new();
    std::io::stdin().read_to_string(&mut input_str)?;

    match cli.command {
        HookCommand::Gate {
            acteon_url,
            agent_id,
            agent_role,
            api_key,
            namespace,
            tenant,
        } => {
            let input: acteon_swarm::hooks::gate::HookInput = serde_json::from_str(&input_str)?;

            let ns = namespace.as_deref().unwrap_or("swarm");
            let tn = tenant.as_deref().unwrap_or("swarm-default");
            let role = agent_role.as_deref().unwrap_or("coding");

            let allowed = acteon_swarm::hooks::gate::dispatch_to_acteon(
                &acteon_url,
                api_key.as_deref(),
                ns,
                tn,
                role,
                &agent_id,
                &input,
            )
            .await?;

            std::process::exit(if allowed { 0 } else { 2 });
        }
        HookCommand::Record {
            tesserai_url,
            agent_id,
        } => {
            // Best-effort: parse the tool output and store as episodic memory.
            let input: serde_json::Value = serde_json::from_str(&input_str).unwrap_or_default();
            let tool_name = input["tool_name"].as_str().unwrap_or("unknown");

            let config = acteon_swarm::config::TesseraiConnectionConfig {
                endpoint: tesserai_url,
                api_key: None,
                tenant_id: "swarm-default".into(),
            };

            if let Ok(client) = acteon_swarm::memory::TesseraiClient::new(&config) {
                let _ = acteon_swarm::memory::semantic::record_action(
                    &client,
                    &agent_id,
                    tool_name,
                    &format!("Used tool: {tool_name}"),
                    vec![tool_name.to_lowercase()],
                    Some(input["tool_input"].clone()),
                )
                .await;
            }

            // Always exit 0 — recording failures should not block the agent.
            std::process::exit(0);
        }
        HookCommand::Complete {
            acteon_url: _,
            tesserai_url,
            agent_id,
        } => {
            // Mark session as complete in TesseraiDB.
            let _ =
                acteon_swarm::hooks::complete::handle_session_complete(&tesserai_url, &agent_id)
                    .await;

            // Always exit 0.
            std::process::exit(0);
        }
    }
}
