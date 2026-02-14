use acteon_ops::OpsClient;
use acteon_ops::acteon_client::AuditQuery;
use clap::Args;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct AuditArgs {
    /// Filter by tenant.
    #[arg(long)]
    pub tenant: Option<String>,
    /// Filter by namespace.
    #[arg(long)]
    pub namespace: Option<String>,
    /// Filter by provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// Filter by action type.
    #[arg(long, name = "type")]
    pub action_type: Option<String>,
    /// Maximum records to return.
    #[arg(long, default_value = "20")]
    pub limit: u32,
}

pub async fn run(ops: &OpsClient, args: &AuditArgs, format: &OutputFormat) -> anyhow::Result<()> {
    let query = AuditQuery {
        tenant: args.tenant.clone(),
        namespace: args.namespace.clone(),
        provider: args.provider.clone(),
        action_type: args.action_type.clone(),
        outcome: None,
        limit: Some(args.limit),
        offset: None,
    };

    let page = ops.client().query_audit(&query).await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&page)?);
        }
        OutputFormat::Text => {
            println!(
                "Total: {} records (showing {})",
                page.total,
                page.records.len()
            );
            for rec in &page.records {
                println!(
                    "  [{ts}] {action_type} -> {provider} | {verdict} ({outcome}) [{id}]",
                    ts = rec.dispatched_at,
                    action_type = rec.action_type,
                    provider = rec.provider,
                    verdict = rec.verdict,
                    outcome = rec.outcome,
                    id = &rec.action_id[..8.min(rec.action_id.len())],
                );
            }
        }
    }

    Ok(())
}
