use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{AuditQuery, ReplayQuery};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct AuditArgs {
    #[command(subcommand)]
    pub command: AuditCommand,
}

#[derive(Subcommand, Debug)]
pub enum AuditCommand {
    /// Query the audit trail.
    Query {
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by provider.
        #[arg(long)]
        provider: Option<String>,
        /// Filter by action type.
        #[arg(long, name = "type")]
        action_type: Option<String>,
        /// Maximum records to return.
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// Get a single audit record by action ID.
    Get {
        /// Action ID.
        action_id: String,
    },
    /// Replay a single action from the audit trail.
    Replay {
        /// Action ID.
        action_id: String,
    },
    /// Replay multiple actions matching filters.
    ReplayBulk {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Filter by provider.
        #[arg(long)]
        provider: Option<String>,
        /// Filter by action type.
        #[arg(long, name = "type")]
        action_type: Option<String>,
        /// Maximum records to replay.
        #[arg(long, default_value = "50")]
        limit: u32,
    },
}

pub async fn run(ops: &OpsClient, args: &AuditArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        AuditCommand::Query {
            tenant,
            namespace,
            provider,
            action_type,
            limit,
        } => {
            run_query(
                ops,
                tenant.as_ref(),
                namespace.as_ref(),
                provider.as_ref(),
                action_type.as_ref(),
                *limit,
                format,
            )
            .await
        }
        AuditCommand::Get { action_id } => run_get(ops, action_id, format).await,
        AuditCommand::Replay { action_id } => run_replay(ops, action_id, format).await,
        AuditCommand::ReplayBulk {
            namespace,
            tenant,
            provider,
            action_type,
            limit,
        } => {
            run_replay_bulk(
                ops,
                namespace.as_ref(),
                tenant.as_ref(),
                provider.as_ref(),
                action_type.as_ref(),
                *limit,
                format,
            )
            .await
        }
    }
}

async fn run_query(
    ops: &OpsClient,
    tenant: Option<&String>,
    namespace: Option<&String>,
    provider: Option<&String>,
    action_type: Option<&String>,
    limit: u32,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let query = AuditQuery {
        tenant: tenant.cloned(),
        namespace: namespace.cloned(),
        provider: provider.cloned(),
        action_type: action_type.cloned(),
        outcome: None,
        limit: Some(limit),
        offset: None,
    };

    let page = ops.query_audit(query).await?;

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&page)?);
        }
        OutputFormat::Text => {
            info!(
                total = page.total,
                showing = page.records.len(),
                "Audit query results"
            );
            for rec in &page.records {
                info!(
                    timestamp = %rec.dispatched_at,
                    action_type = %rec.action_type,
                    provider = %rec.provider,
                    verdict = %rec.verdict,
                    outcome = %rec.outcome,
                    id = %&rec.action_id[..8.min(rec.action_id.len())],
                    "Audit record"
                );
            }
        }
    }
    Ok(())
}

async fn run_get(ops: &OpsClient, action_id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let record = ops.get_audit_record(action_id).await?;

    match record {
        Some(rec) => match format {
            OutputFormat::Json => {
                info!("{}", serde_json::to_string_pretty(&rec)?);
            }
            OutputFormat::Text => {
                info!(action_id = %rec.action_id, "Audit record details");
                info!(namespace = %rec.namespace, "  Namespace");
                info!(tenant = %rec.tenant, "  Tenant");
                info!(provider = %rec.provider, "  Provider");
                info!(action_type = %rec.action_type, "  Action Type");
                info!(verdict = %rec.verdict, "  Verdict");
                info!(outcome = %rec.outcome, "  Outcome");
                info!(duration_ms = rec.duration_ms, "  Duration");
                info!(dispatched_at = %rec.dispatched_at, "  Dispatched");
                if let Some(ref rule) = rec.matched_rule {
                    info!(matched_rule = %rule, "  Matched Rule");
                }
            }
        },
        None => {
            warn!(action_id = %action_id, "Audit record not found");
        }
    }
    Ok(())
}

async fn run_replay(ops: &OpsClient, action_id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let result = ops.replay_action(action_id).await?;

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Text => {
            if result.success {
                info!(
                    original_id = %result.original_action_id,
                    new_id = %result.new_action_id,
                    "Replay succeeded"
                );
            } else {
                let err = result.error.as_deref().unwrap_or("unknown");
                warn!(
                    original_id = %result.original_action_id,
                    error = %err,
                    "Replay failed"
                );
            }
        }
    }
    Ok(())
}

async fn run_replay_bulk(
    ops: &OpsClient,
    namespace: Option<&String>,
    tenant: Option<&String>,
    provider: Option<&String>,
    action_type: Option<&String>,
    limit: u32,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let query = ReplayQuery {
        namespace: namespace.cloned(),
        tenant: tenant.cloned(),
        provider: provider.cloned(),
        action_type: action_type.cloned(),
        limit: Some(limit),
        ..Default::default()
    };

    let summary = ops.replay_audit(query).await?;

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&summary)?);
        }
        OutputFormat::Text => {
            info!(
                replayed = summary.replayed,
                failed = summary.failed,
                skipped = summary.skipped,
                "Replay complete"
            );
            for r in &summary.results {
                if r.success {
                    info!(
                        original_id = %r.original_action_id,
                        new_id = %r.new_action_id,
                        "  OK"
                    );
                } else {
                    let err = r.error.as_deref().unwrap_or("unknown");
                    warn!(
                        original_id = %r.original_action_id,
                        error = %err,
                        "  ERR"
                    );
                }
            }
        }
    }
    Ok(())
}
