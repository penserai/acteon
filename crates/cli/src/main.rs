//! Acteon CLI
//!
//! A command-line interface for interacting with the Acteon action gateway.

mod commands;

use acteon_ops::{OpsClient, OpsConfig};
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt};

/// Acteon CLI — interact with the Acteon action gateway.
#[derive(Parser, Debug)]
#[command(name = "acteon", version, about)]
struct Cli {
    /// Acteon server endpoint URL.
    #[arg(
        long,
        env = "ACTEON_ENDPOINT",
        default_value = "http://localhost:8080",
        global = true
    )]
    endpoint: String,

    /// API key for authentication.
    #[arg(long, env = "ACTEON_API_KEY", global = true)]
    api_key: Option<String>,

    /// Output format.
    #[arg(long, default_value = "text", global = true)]
    format: OutputFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Check gateway health.
    Health,
    /// Dispatch an action through the gateway.
    Dispatch(commands::dispatch::DispatchArgs),
    /// Query and replay audit records.
    Audit(commands::audit::AuditArgs),
    /// Manage routing rules.
    Rules(commands::rules::RulesArgs),
    /// Manage stateful events.
    Events(commands::events::EventsArgs),
    /// Manage action chains.
    Chains(commands::chains::ChainsArgs),
    /// Manage recurring actions.
    Recurring(commands::recurring::RecurringArgs),
    /// Manage tenant quotas.
    Quotas(commands::quotas::QuotasArgs),
    /// Manage retention policies.
    Retention(commands::retention::RetentionArgs),
    /// Manage templates and profiles.
    Templates(commands::templates::TemplatesArgs),
    /// Manage WASM plugins.
    Plugins(commands::plugins::PluginsArgs),
    /// Manage event groups.
    Groups(commands::groups::GroupsArgs),
    /// Manage dead letter queue.
    Dlq(commands::dlq::DlqArgs),
    /// Check compliance status.
    Compliance(commands::compliance::ComplianceArgs),
    /// Provider health information.
    Providers(commands::providers::ProvidersArgs),
    /// List pending approvals.
    Approvals(commands::approvals::ApprovalsArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let config = OpsConfig::new(&cli.endpoint);
    let config = match cli.api_key {
        Some(ref key) => config.with_api_key(key),
        None => config,
    };
    let ops = OpsClient::from_config(&config)?;

    match cli.command {
        Command::Health => commands::health::run(&ops).await,
        Command::Dispatch(args) => commands::dispatch::run(&ops, &args, &cli.format).await,
        Command::Audit(args) => commands::audit::run(&ops, &args, &cli.format).await,
        Command::Rules(args) => commands::rules::run(&ops, &args, &cli.format).await,
        Command::Events(args) => commands::events::run(&ops, &args, &cli.format).await,
        Command::Chains(args) => commands::chains::run(&ops, &args, &cli.format).await,
        Command::Recurring(args) => commands::recurring::run(&ops, &args, &cli.format).await,
        Command::Quotas(args) => commands::quotas::run(&ops, &args, &cli.format).await,
        Command::Retention(args) => commands::retention::run(&ops, &args, &cli.format).await,
        Command::Templates(args) => commands::templates::run(&ops, &args, &cli.format).await,
        Command::Plugins(args) => commands::plugins::run(&ops, &args, &cli.format).await,
        Command::Groups(args) => commands::groups::run(&ops, &args, &cli.format).await,
        Command::Dlq(args) => commands::dlq::run(&ops, &args, &cli.format).await,
        Command::Compliance(args) => commands::compliance::run(&ops, &args, &cli.format).await,
        Command::Providers(args) => commands::providers::run(&ops, &args, &cli.format).await,
        Command::Approvals(args) => commands::approvals::run(&ops, &args, &cli.format).await,
    }
}
