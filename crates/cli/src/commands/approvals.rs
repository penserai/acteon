use acteon_ops::OpsClient;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct ApprovalsArgs {
    #[command(subcommand)]
    pub command: ApprovalsCommand,
}

#[derive(Subcommand, Debug)]
pub enum ApprovalsCommand {
    /// List pending approvals.
    List {
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
}

pub async fn run(
    ops: &OpsClient,
    args: &ApprovalsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ApprovalsCommand::List { namespace, tenant } => {
            let resp = ops.list_approvals(namespace, tenant).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(count = resp.count, "Pending approvals");
                    for a in &resp.approvals {
                        let msg = a.message.as_deref().unwrap_or("");
                        info!(
                            token = %&a.token[..8.min(a.token.len())],
                            status = %a.status,
                            rule = %a.rule,
                            expires_at = %a.expires_at,
                            message = %msg,
                            "Approval"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
