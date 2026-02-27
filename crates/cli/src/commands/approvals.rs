use acteon_ops::OpsClient;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("{} pending approvals:", resp.count);
                    for a in &resp.approvals {
                        let msg = a.message.as_deref().unwrap_or("");
                        println!(
                            "  {token} | {status} | rule: {rule} | expires: {expires} {msg}",
                            token = &a.token[..8.min(a.token.len())],
                            status = a.status,
                            rule = a.rule,
                            expires = a.expires_at,
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
