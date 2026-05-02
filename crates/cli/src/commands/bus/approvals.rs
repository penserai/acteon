use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    BusApprovalDecisionRequest, BusApprovalStatus, ListBusApprovalsParams,
};
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
    /// List parked approvals for a tenant.
    List {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        /// Filter by status: `pending`, `approved`, `rejected`, `expired`.
        #[arg(long)]
        status: Option<ApprovalStatusKind>,
        #[arg(long)]
        conversation_id: Option<String>,
    },
    /// Fetch a single approval by id.
    Get {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        approval_id: String,
    },
    /// Approve a parked tool-call. Produces the original envelope.
    Approve {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        approval_id: String,
        #[arg(long)]
        decided_by: String,
        #[arg(long)]
        decision_note: Option<String>,
    },
    /// Reject a parked tool-call. No Kafka record is produced.
    Reject {
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        approval_id: String,
        #[arg(long)]
        decided_by: String,
        #[arg(long)]
        decision_note: Option<String>,
    },
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ApprovalStatusKind {
    Pending,
    Approved,
    Rejected,
    Expired,
}

impl From<ApprovalStatusKind> for BusApprovalStatus {
    fn from(s: ApprovalStatusKind) -> Self {
        match s {
            ApprovalStatusKind::Pending => Self::Pending,
            ApprovalStatusKind::Approved => Self::Approved,
            ApprovalStatusKind::Rejected => Self::Rejected,
            ApprovalStatusKind::Expired => Self::Expired,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    ops: &OpsClient,
    args: &ApprovalsArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        ApprovalsCommand::List {
            namespace,
            tenant,
            status,
            conversation_id,
        } => {
            let params = ListBusApprovalsParams {
                status: status.clone().map(BusApprovalStatus::from),
                conversation_id: conversation_id.clone(),
            };
            let resp = ops
                .client()
                .list_bus_approvals(namespace, tenant, &params)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(count = resp.count, "Bus approvals");
                    for a in &resp.approvals {
                        info!(
                            approval_id = %a.approval_id,
                            status = ?a.status,
                            envelope_kind = %a.envelope_kind,
                            conversation_id = %a.conversation_id,
                            expires_at = %a.expires_at,
                            "Approval"
                        );
                    }
                }
            }
        }
        ApprovalsCommand::Get {
            namespace,
            tenant,
            approval_id,
        } => {
            let a = ops
                .client()
                .get_bus_approval(namespace, tenant, approval_id)
                .await?;
            info!("{}", serde_json::to_string_pretty(&a)?);
        }
        ApprovalsCommand::Approve {
            namespace,
            tenant,
            approval_id,
            decided_by,
            decision_note,
        } => {
            let req = BusApprovalDecisionRequest {
                decided_by: decided_by.clone(),
                decision_note: decision_note.clone(),
            };
            let resp = ops
                .client()
                .approve_bus_approval(namespace, tenant, approval_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => {
                    info!(
                        approval_id = %resp.approval.approval_id,
                        status = ?resp.approval.status,
                        "Approval approved"
                    );
                    if let Some(receipt) = &resp.receipt {
                        info!(
                            call_id = %receipt.call_id,
                            partition = receipt.partition,
                            offset = receipt.offset,
                            "Produced envelope"
                        );
                    }
                }
            }
        }
        ApprovalsCommand::Reject {
            namespace,
            tenant,
            approval_id,
            decided_by,
            decision_note,
        } => {
            let req = BusApprovalDecisionRequest {
                decided_by: decided_by.clone(),
                decision_note: decision_note.clone(),
            };
            let resp = ops
                .client()
                .reject_bus_approval(namespace, tenant, approval_id, &req)
                .await?;
            match format {
                OutputFormat::Json => info!("{}", serde_json::to_string_pretty(&resp)?),
                OutputFormat::Text => info!(
                    approval_id = %resp.approval.approval_id,
                    status = ?resp.approval.status,
                    "Approval rejected"
                ),
            }
        }
    }
    Ok(())
}
