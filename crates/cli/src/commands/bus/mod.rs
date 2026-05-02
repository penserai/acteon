//! `acteon bus` subcommand tree.
//!
//! Mirrors the agentic-bus HTTP surface (`/v1/bus/*`) so operators
//! and CI scripts can drive every primitive without a custom client.

pub mod agents;
pub mod approvals;
pub mod conversations;
pub mod schemas;
pub mod streams;
pub mod subscriptions;
pub mod tool_calls;
pub mod topics;

use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct BusArgs {
    #[command(subcommand)]
    pub command: BusCommand,
}

#[derive(Subcommand, Debug)]
pub enum BusCommand {
    /// Manage bus topics (Kafka-backed).
    Topics(topics::TopicsArgs),
    /// Manage durable subscriptions and consumer-group state.
    Subscriptions(subscriptions::SubscriptionsArgs),
    /// Manage JSON Schema registry and topic-schema bindings.
    Schemas(schemas::SchemasArgs),
    /// Manage agents (shared inbox topic per `(namespace, tenant)`).
    Agents(agents::AgentsArgs),
    /// Manage conversations (per-thread FIFO on shared events topic).
    Conversations(conversations::ConversationsArgs),
    /// Post and look up tool-call / tool-result envelopes.
    #[command(name = "tool-calls")]
    ToolCalls(tool_calls::ToolCallsArgs),
    /// Stream-chunk envelopes plus consume URLs.
    Streams(streams::StreamsArgs),
    /// HITL pre-publish approvals for parked tool-calls.
    Approvals(approvals::ApprovalsArgs),
}

pub async fn run(ops: &OpsClient, args: &BusArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        BusCommand::Topics(a) => topics::run(ops, a, format).await,
        BusCommand::Subscriptions(a) => subscriptions::run(ops, a, format).await,
        BusCommand::Schemas(a) => schemas::run(ops, a, format).await,
        BusCommand::Agents(a) => agents::run(ops, a, format).await,
        BusCommand::Conversations(a) => conversations::run(ops, a, format).await,
        BusCommand::ToolCalls(a) => tool_calls::run(ops, a, format).await,
        BusCommand::Streams(a) => streams::run(ops, a, format).await,
        BusCommand::Approvals(a) => approvals::run(ops, a, format).await,
    }
}

/// Helper for parsing `key=value` flag inputs into a `HashMap`.
pub(crate) fn parse_kv(s: &str) -> anyhow::Result<(String, String)> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("expected key=value, got {s:?}"))?;
    Ok((k.to_string(), v.to_string()))
}

/// Helper for parsing JSON value inputs from `--payload @file.json` or
/// `--payload '{"x": 1}'` style flags.
pub(crate) fn parse_json_arg(input: &str) -> anyhow::Result<serde_json::Value> {
    if let Some(path) = input.strip_prefix('@') {
        let bytes =
            std::fs::read(path).map_err(|e| anyhow::anyhow!("failed to read {path}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("invalid JSON in {path}: {e}"))
    } else {
        serde_json::from_str(input).map_err(|e| anyhow::anyhow!("invalid JSON: {e}"))
    }
}
