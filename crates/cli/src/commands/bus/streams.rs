use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    BusStreamEndStatus, BusStreamItem, PostBusStreamChunk, PostBusStreamEnd,
};
use clap::{Args, Subcommand};
use futures::StreamExt;
use tracing::{info, warn};

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct StreamsArgs {
    #[command(subcommand)]
    pub command: StreamsCommand,
}

#[derive(Subcommand, Debug)]
pub enum StreamsCommand {
    /// Append a stream-chunk envelope.
    Chunk {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        stream_id: String,
        #[arg(long)]
        chunk_seq: i64,
        /// JSON body, or `@path/to/file.json`.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long = "metadata", value_parser = parse_kv)]
        metadata: Vec<(String, String)>,
    },
    /// Append the terminal `stream_end` marker.
    End {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        stream_id: String,
        #[arg(long)]
        chunk_seq: i64,
        /// `complete`, `aborted`, or `error`.
        #[arg(long)]
        status: StreamEndStatusKind,
        #[arg(long)]
        error_message: Option<String>,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long = "metadata", value_parser = parse_kv)]
        metadata: Vec<(String, String)>,
    },
    /// Print the SSE consume URL for a stream. Pipe into `curl -N
    /// --header 'accept: text/event-stream' "$(...)"` to tail it.
    ConsumeUrl {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        stream_id: String,
    },
    /// Tail a single stream over SSE. Prints each chunk as JSONL on
    /// stdout and exits when the terminal `stream_end` lands.
    Tail {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        stream_id: String,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum StreamEndStatusKind {
    Complete,
    Aborted,
    Error,
}

impl From<StreamEndStatusKind> for BusStreamEndStatus {
    fn from(s: StreamEndStatusKind) -> Self {
        match s {
            StreamEndStatusKind::Complete => Self::Complete,
            StreamEndStatusKind::Aborted => Self::Aborted,
            StreamEndStatusKind::Error => Self::Error,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn run(ops: &OpsClient, args: &StreamsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        StreamsCommand::Chunk {
            namespace,
            tenant,
            conversation_id,
            stream_id,
            chunk_seq,
            body,
            sender,
            metadata,
        } => {
            let body = body
                .as_deref()
                .map(parse_json_arg)
                .transpose()?
                .unwrap_or(serde_json::Value::Null);
            let req = PostBusStreamChunk {
                stream_id: stream_id.clone(),
                chunk_seq: *chunk_seq,
                body,
                sender: sender.clone(),
                metadata: metadata.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let receipt = ops
                .client()
                .post_bus_stream_chunk(namespace, tenant, conversation_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    stream_id = %receipt.stream_id,
                    chunk_seq = receipt.chunk_seq,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    cursor = %receipt.cursor,
                    "Stream chunk produced"
                ),
            }
        }
        StreamsCommand::End {
            namespace,
            tenant,
            conversation_id,
            stream_id,
            chunk_seq,
            status,
            error_message,
            sender,
            metadata,
        } => {
            let req = PostBusStreamEnd {
                stream_id: stream_id.clone(),
                chunk_seq: *chunk_seq,
                status: BusStreamEndStatus::from(status.clone()),
                error_message: error_message.clone(),
                sender: sender.clone(),
                metadata: metadata.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let receipt = ops
                .client()
                .post_bus_stream_end(namespace, tenant, conversation_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    stream_id = %receipt.stream_id,
                    chunk_seq = receipt.chunk_seq,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    "Stream end produced"
                ),
            }
        }
        StreamsCommand::ConsumeUrl {
            namespace,
            tenant,
            conversation_id,
            stream_id,
        } => {
            let url =
                ops.client()
                    .bus_stream_consume_url(namespace, tenant, conversation_id, stream_id);
            // Print the URL to stdout so it can be piped without log
            // formatting noise.
            println!("{url}");
        }
        StreamsCommand::Tail {
            namespace,
            tenant,
            conversation_id,
            stream_id,
        } => {
            let mut stream = ops
                .client()
                .consume_bus_stream(namespace, tenant, conversation_id, stream_id)
                .await?;
            while let Some(item) = stream.next().await {
                match item? {
                    BusStreamItem::Chunk(chunk) => {
                        println!("{}", serde_json::to_string(&chunk)?);
                    }
                    BusStreamItem::End(end) => {
                        println!("{}", serde_json::to_string(&end)?);
                        info!(stream_id = %end.stream_id, status = ?end.status, "Stream ended");
                        break;
                    }
                    BusStreamItem::Error { message } => {
                        warn!(error = %message, "bus.stream.error");
                    }
                    BusStreamItem::KeepAlive => {}
                }
            }
            // Suppress unused-variable warning when the format flag is
            // unused — the tail loop emits to stdout regardless of mode.
            let _ = format;
        }
    }
    Ok(())
}
