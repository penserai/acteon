use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    AppendBusConversationMessage, BusConversationFilter, BusConversationTransition,
    RegisterBusConversation, ReplayBusConversationParams, UpdateBusConversation,
};
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;
use crate::commands::bus::{parse_json_arg, parse_kv};

#[derive(Args, Debug)]
pub struct ConversationsArgs {
    #[command(subcommand)]
    pub command: ConversationsCommand,
}

#[derive(Subcommand, Debug)]
pub enum ConversationsCommand {
    /// List conversations, optionally filtered.
    List {
        #[arg(long)]
        namespace: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        /// `active`, `resolved`, or `archived`.
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        participant: Option<String>,
    },
    /// Fetch a single conversation.
    Get {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
    },
    /// Register a conversation. First in a tenant auto-creates the
    /// shared events topic.
    Create {
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        title: Option<String>,
        /// Repeat for multiple participants.
        #[arg(long = "participant")]
        participants: Vec<String>,
        #[arg(long)]
        events_topic: Option<String>,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Vec<(String, String)>,
    },
    /// Update mutable fields. Use `transition` for state changes.
    Update {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long = "participant")]
        participants: Option<Vec<String>>,
        #[arg(long = "label", value_parser = parse_kv)]
        labels: Option<Vec<(String, String)>>,
    },
    /// Delete a conversation. Shared events topic is preserved.
    Delete {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
    },
    /// Drive the conversation through its state machine.
    Transition {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        /// `resolve`, `reopen`, or `archive`.
        #[arg(long)]
        transition: TransitionKind,
    },
    /// Append a message to the conversation thread (per-thread FIFO).
    Append {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        /// JSON payload, or `@path/to/file.json`.
        #[arg(long)]
        payload: String,
        #[arg(long)]
        sender: Option<String>,
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
    },
    /// Replay the message history for a conversation.
    Replay {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        conversation_id: String,
        /// Starting position label; ignored when `--cursor` is set.
        #[arg(long)]
        from: Option<String>,
        /// Resume token from a prior response.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum TransitionKind {
    Resolve,
    Reopen,
    Archive,
}

impl From<TransitionKind> for BusConversationTransition {
    fn from(t: TransitionKind) -> Self {
        match t {
            TransitionKind::Resolve => Self::Resolve,
            TransitionKind::Reopen => Self::Reopen,
            TransitionKind::Archive => Self::Archive,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    ops: &OpsClient,
    args: &ConversationsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ConversationsCommand::List {
            namespace,
            tenant,
            state,
            participant,
        } => {
            let filter = BusConversationFilter {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                state: state.clone(),
                participant: participant.clone(),
            };
            let resp = ops.client().list_bus_conversations(&filter).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus conversations");
                    for c in &resp.conversations {
                        info!(
                            conversation_id = %c.conversation_id,
                            namespace = %c.namespace,
                            tenant = %c.tenant,
                            state = ?c.state,
                            participants = ?c.participants,
                            "Conversation"
                        );
                    }
                }
            }
        }
        ConversationsCommand::Get {
            namespace,
            tenant,
            conversation_id,
        } => {
            let c = ops
                .client()
                .get_bus_conversation(namespace, tenant, conversation_id)
                .await?;
            info!("{}", serde_json::to_string_pretty(&c)?);
        }
        ConversationsCommand::Create {
            conversation_id,
            namespace,
            tenant,
            title,
            participants,
            events_topic,
            labels,
        } => {
            let req = RegisterBusConversation {
                conversation_id: conversation_id.clone(),
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                title: title.clone(),
                participants: participants.clone(),
                events_topic: events_topic.clone(),
                labels: labels.iter().cloned().collect::<HashMap<_, _>>(),
            };
            let c = ops.client().register_bus_conversation(&req).await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&c)?),
                OutputFormat::Text => info!(
                    conversation_id = %c.conversation_id,
                    events_topic = %c.events_topic,
                    "Conversation created"
                ),
            }
        }
        ConversationsCommand::Update {
            namespace,
            tenant,
            conversation_id,
            title,
            participants,
            labels,
        } => {
            let req = UpdateBusConversation {
                title: title.clone(),
                participants: participants.clone(),
                labels: labels
                    .as_ref()
                    .map(|kvs| kvs.iter().cloned().collect::<HashMap<_, _>>()),
            };
            let c = ops
                .client()
                .update_bus_conversation(namespace, tenant, conversation_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&c)?),
                OutputFormat::Text => info!(
                    conversation_id = %c.conversation_id,
                    "Conversation updated"
                ),
            }
        }
        ConversationsCommand::Delete {
            namespace,
            tenant,
            conversation_id,
        } => {
            ops.client()
                .delete_bus_conversation(namespace, tenant, conversation_id)
                .await?;
            info!(conversation_id = %conversation_id, "Conversation deleted");
        }
        ConversationsCommand::Transition {
            namespace,
            tenant,
            conversation_id,
            transition,
        } => {
            let c = ops
                .client()
                .transition_bus_conversation(
                    namespace,
                    tenant,
                    conversation_id,
                    BusConversationTransition::from(transition.clone()),
                )
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&c)?),
                OutputFormat::Text => info!(
                    conversation_id = %c.conversation_id,
                    state = ?c.state,
                    "Conversation transitioned"
                ),
            }
        }
        ConversationsCommand::Append {
            namespace,
            tenant,
            conversation_id,
            payload,
            sender,
            headers,
        } => {
            let payload = parse_json_arg(payload)?;
            let req = AppendBusConversationMessage {
                payload,
                sender: sender.clone(),
                headers: headers.iter().cloned().collect(),
            };
            let receipt = ops
                .client()
                .append_bus_conversation_message(namespace, tenant, conversation_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&receipt)?),
                OutputFormat::Text => info!(
                    conversation_id = %receipt.conversation_id,
                    events_topic = %receipt.events_topic,
                    partition = receipt.partition,
                    offset = receipt.offset,
                    "Message appended"
                ),
            }
        }
        ConversationsCommand::Replay {
            namespace,
            tenant,
            conversation_id,
            from,
            cursor,
            limit,
            timeout_ms,
        } => {
            let params = ReplayBusConversationParams {
                from: from.clone(),
                limit: *limit,
                timeout_ms: *timeout_ms,
                cursor: cursor.clone(),
            };
            let resp = ops
                .client()
                .replay_bus_conversation_messages(namespace, tenant, conversation_id, &params)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(
                        conversation_id = %resp.conversation_id,
                        events_topic = %resp.events_topic,
                        message_count = resp.messages.len(),
                        exit_reason = ?resp.exit_reason,
                        cursor = resp.cursor.as_deref().unwrap_or(""),
                        "Conversation replay"
                    );
                    for m in &resp.messages {
                        info!(
                            partition = m.partition,
                            offset = m.offset,
                            timestamp = %m.timestamp,
                            payload = %m.payload,
                            "Message"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
