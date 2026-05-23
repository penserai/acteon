//! Acteon MCP Server
//!
//! Exposes the Acteon action gateway to LLM agents via the
//! Model Context Protocol (MCP). Runs over `stdio` transport.

use acteon_ops::{OpsClient, OpsConfig};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt};

mod prompts;
mod resources;
mod server;
mod tools;

use server::ActeonMcpServer;

/// Acteon MCP Server — expose the action gateway to AI agents.
#[derive(Parser, Debug)]
#[command(name = "acteon-mcp-server", version, about)]
struct Args {
    /// Acteon server endpoint URL.
    #[arg(long, env = "ACTEON_ENDPOINT", default_value = "http://localhost:8080")]
    endpoint: String,

    /// API key for authentication.
    #[arg(long, env = "ACTEON_API_KEY")]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // MCP servers MUST NOT write to stdout (that's the transport).
    // Direct logs to stderr instead.
    fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args = Args::parse();

    let config = OpsConfig::new(&args.endpoint);
    let config = match args.api_key {
        Some(ref key) => config.with_api_key(key),
        None => config,
    };

    let ops = OpsClient::from_config(&config)?;

    tracing::info!(endpoint = %args.endpoint, "starting Acteon MCP server");

    let service = ActeonMcpServer::new(ops).serve(stdio()).await?;

    service.waiting().await?;

    Ok(())
}
