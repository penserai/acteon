use acteon_ops::OpsClient;
use acteon_ops::acteon_client::EventQuery;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} events:", resp.count);
                    for event in &resp.events {
                        let updated = event.updated_at.as_deref().unwrap_or("?");
                        println!(
                            "  {fingerprint} | {state} | updated {updated}",
                            fingerprint = event.fingerprint,
                            state = event.state,
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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!(
                        "Event {fp}: {prev} -> {next} (notify: {notify})",
                        fp = resp.fingerprint,
                        prev = resp.previous_state,
                        next = resp.new_state,
                        notify = resp.notify,
                    );
                }
            }
        }
    }
    Ok(())
}
