use acteon_ops::OpsClient;
use acteon_ops::acteon_client::EventQuery;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct EventsArgs {
    #[command(subcommand)]
    pub command: EventsCommand,
}

#[derive(Subcommand, Debug)]
pub enum EventsCommand {
    /// List stateful events.
    List {
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
        /// Filter by state.
        #[arg(long)]
        status: Option<String>,
    },
    /// Transition an event to a new state.
    Transition {
        /// Event fingerprint.
        fingerprint: String,
        /// Target state (e.g. "acknowledged", "resolved").
        #[arg(long)]
        to: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
}

pub async fn run(ops: &OpsClient, args: &EventsArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        EventsCommand::List {
            namespace,
            tenant,
            status,
        } => {
            let query = EventQuery {
                namespace: namespace.clone(),
                tenant: tenant.clone(),
                status: status.clone(),
                limit: Some(50),
            };
            let resp = ops.client().list_events(&query).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.count, "Events");
                    for event in &resp.events {
                        let updated = event.updated_at.as_deref().unwrap_or("?");
                        info!(
                            fingerprint = %event.fingerprint,
                            state = %event.state,
                            updated = %updated,
                            "Event"
                        );
                    }
                }
            }
        }
        EventsCommand::Transition {
            fingerprint,
            to,
            namespace,
            tenant,
        } => {
            let resp = ops
                .client()
                .transition_event(fingerprint, to, namespace, tenant)
                .await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(
                        fingerprint = %resp.fingerprint,
                        previous_state = %resp.previous_state,
                        new_state = %resp.new_state,
                        notify = resp.notify,
                        "Event transitioned"
                    );
                }
            }
        }
    }
    Ok(())
}
