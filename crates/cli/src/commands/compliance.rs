use acteon_ops::OpsClient;
use acteon_ops::acteon_client::VerifyHashChainRequest;
use clap::{Args, Subcommand};

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
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Compliance Mode:    {}", resp.mode);
                    println!("Sync Audit Writes:  {}", resp.sync_audit_writes);
                    println!("Immutable Audit:    {}", resp.immutable_audit);
                    println!("Hash Chain:         {}", resp.hash_chain);
                }
            }
        }
        ComplianceCommand::Verify { data } => {
            let value = parse_json_data(data)?;
            let req: VerifyHashChainRequest = serde_json::from_value(value)?;
            let resp = ops.verify_audit_chain(&req).await?;
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                }
                OutputFormat::Text => {
                    println!("Valid:           {}", resp.valid);
                    println!("Records Checked: {}", resp.records_checked);
                    if let Some(ref broken) = resp.first_broken_at {
                        println!("First Broken:    {broken}");
                    }
                    if let Some(ref first) = resp.first_record_id {
                        println!("First Record:    {first}");
                    }
                    if let Some(ref last) = resp.last_record_id {
                        println!("Last Record:     {last}");
                    }
                }
            }
        }
    }
    Ok(())
}
