use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    BusToolResultLookupParams, BusToolResultStatus, PostBusToolCall, PostBusToolCallOutcome,
    PostBusToolResult,
};
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct ToolCallsArgs {
    #[command(subcommand)]
    pub command: ToolCallsCommand,
}

#[derive(Subcommand, Debug)]
pub enum ToolCallsCommand {
    /// Post a tool-call envelope. With `--require-approval`, the
    /// server parks it under a HITL approval row and returns 202.
    Post {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        call_id: String,
        /// Tool identifier, e.g. `billing.charge`.
        #[arg(long)]
        tool: String,
        /// Tool arguments as JSON, or `@path/to/args.json`.
        #[arg(long)]
        arguments: Option<String>,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long)]
        correlation_id: Option<String>,
        /// Conversation id where the result is expected to land.
        #[arg(long)]
        reply_to: Option<String>,
        #[arg(long = "metadata", value_parser = parse_kv)]
        metadata: Vec<(String, String)>,
        /// Park behind a human-in-the-loop approval.
        #[arg(long)]
        require_approval: bool,
        /// Free-form rationale shown to the operator.
        #[arg(long)]
        approval_reason: Option<String>,
        /// Override default 24h TTL (max 7d).
        #[arg(long)]
        approval_ttl_ms: Option<u64>,
    },
    /// Post a tool-result envelope.
    PostResult {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        call_id: String,
        /// `ok`, `error`, or `canceled`.
        #[arg(long)]
        status: ToolResultStatusKind,
        /// JSON output, or `@path/to/file.json`.
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        error_message: Option<String>,
        #[arg(long)]
        correlation_id: Option<String>,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long = "metadata", value_parser = parse_kv)]
        metadata: Vec<(String, String)>,
    },
    /// Look up a tool result by `call_id`. Strongly recommend passing
    /// `--cursor` from the originating call's receipt.
    Lookup {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        call_id: String,
        /// Required: which conversation to scan.
        #[arg(long)]
        conversation_id: String,
        /// Resume cursor from the call receipt.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ToolResultStatusKind {
    Ok,
    Error,
    Canceled,
}

impl From<ToolResultStatusKind> for BusToolResultStatus {
    fn from(s: ToolResultStatusKind) -> Self {
        match s {
            ToolResultStatusKind::Ok => Self::Ok,
            ToolResultStatusKind::Error => Self::Error,
            ToolResultStatusKind::Canceled => Self::Canceled,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    ops: &OpsClient,
    args: &ToolCallsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ToolCallsCommand::Post {
            namespace,
            tenant,
            conversation_id,
            call_id,
            tool,
            arguments,
            sender,
            correlation_id,
            reply_to,
            metadata,
            require_approval,
            approval_reason,
            approval_ttl_ms,
        } => {
            let arguments = arguments
                .as_deref()
                .map(parse_json_arg)
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            let req = PostBusToolCall {
                call_id: call_id.clone(),
                tool: tool.clone(),
                arguments,
                correlation_id: correlation_id.clone(),
                reply_to: reply_to.clone(),
                sender: sender.clone(),
                metadata: metadata.iter().cloned().collect::<HashMap<_, _>>(),
                require_approval: *require_approval,
                approval_reason: approval_reason.clone(),
                approval_ttl_ms: *approval_ttl_ms,
            };
            let outcome = ops
                .client()
                .post_bus_tool_call(namespace, tenant, conversation_id, &req)
                .await?;
            match (format, outcome) {
                (OutputFormat::Json, PostBusToolCallOutcome::Produced(r)) => {
                    info!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "outcome": "produced",
                            "receipt": r,
                        }))?
                    );
                }
                (OutputFormat::Json, PostBusToolCallOutcome::Parked(p)) => {
                    info!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "outcome": "parked",
                            "approval": p,
                        }))?
                    );
                }
                (OutputFormat::Text, PostBusToolCallOutcome::Produced(r)) => info!(
                    call_id = %r.call_id,
                    events_topic = %r.events_topic,
                    partition = r.partition,
                    offset = r.offset,
                    cursor = %r.cursor,
                    "Tool-call produced"
                ),
                (OutputFormat::Text, PostBusToolCallOutcome::Parked(p)) => info!(
                    approval_id = %p.approval_id,
                    correlation_token = %p.correlation_token,
                    expires_at = %p.expires_at,
                    "Tool-call parked for approval"
                ),
            }
        }
        ToolCallsCommand::PostResult {
            namespace,
            tenant,
            conversation_id,
            call_id,
            status,
            output,
            error_message,
            correlation_id,
            sender,
            metadata,
        } => {
            let output = output
                .as_deref()
                .map(parse_json_arg)
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            let req = PostBusToolResult {
                call_id: call_id.clone(),
                status: BusToolResultStatus::from(status.clone()),
                output,
                error_message: error_message.clone(),
                correlation_id: correlation_id.clone(),
                sender: sender.clone(),
                metadata: metadata.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let receipt = ops
                .client()
                .post_bus_tool_result(namespace, tenant, conversation_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    call_id = %receipt.call_id,
                    events_topic = %receipt.events_topic,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    "Tool-result produced"
                ),
            }
        }
        ToolCallsCommand::Lookup {
            namespace,
            tenant,
            call_id,
            conversation_id,
            cursor,
            timeout_ms,
        } => {
            let params = BusToolResultLookupParams {
                conversation_id: conversation_id.clone(),
                cursor: cursor.clone(),
                timeout_ms: *timeout_ms,
            };
            let lookup = ops
                .client()
                .lookup_bus_tool_result(namespace, tenant, call_id, &params)
                .await?;
            info!("{}", serde_json::to_string_pretty(&lookup)?);
        }
    }
    Ok(())
}
