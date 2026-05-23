use acteon_ops::OpsClient;
use acteon_ops::acteon_client::VerifyHashChainRequest;
use clap::{Args, Subcommand};
use tracing::info;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct ComplianceArgs {
    #[command(subcommand)]
    pub command: ComplianceCommand,
}

#[derive(Subcommand, Debug)]
pub enum ComplianceCommand {
    /// Show compliance configuration status.
    Status,
    /// Verify audit hash chain integrity.
    Verify {
        /// JSON data (string or @file path).
        #[arg(long)]
        data: String,
    },
}

fn parse_json_data(input: &str) -> anyhow::Result<serde_json::Value> {
    if let Some(path) = input.strip_prefix('@') {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(serde_json::from_str(input)?)
    }
}

pub async fn run(
    ops: &OpsClient,
    args: &ComplianceArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ComplianceCommand::Status => {
            let resp = ops.get_compliance_status().await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(mode = %resp.mode, "Compliance Mode");
                    info!(
                        sync_audit_writes = resp.sync_audit_writes,
                        "Sync Audit Writes"
                    );
                    info!(immutable_audit = resp.immutable_audit, "Immutable Audit");
                    info!(hash_chain = resp.hash_chain, "Hash Chain");
                }
            }
        }
        ComplianceCommand::Verify { data } => {
            let value = parse_json_data(data)?;
            let req: VerifyHashChainRequest = serde_json::from_value(value)?;
            let resp = ops.verify_audit_chain(&req).await?;
            match format {
                OutputFormat::Json => {
                    info!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    info!(valid = resp.valid, "Verification result");
                    info!(records_checked = resp.records_checked, "Records Checked");
                    if let Some(ref broken) = resp.first_broken_at {
                        info!(first_broken_at = %broken, "First Broken");
                    }
                    if let Some(ref first) = resp.first_record_id {
                        info!(first_record_id = %first, "First Record");
                    }
                    if let Some(ref last) = resp.last_record_id {
                        info!(last_record_id = %last, "Last Record");
                    }
                }
            }
        }
    }
    Ok(())
}
