use std::collections::HashMap;

use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{CreateQuotaRequest, UpdateQuotaRequest};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct QuotasArgs {
    #[command(subcommand)]
    pub command: QuotasCommand,
}

#[derive(Subcommand, Debug)]
pub enum QuotasCommand {
    /// List quota policies.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Filter by provider scope. Pass the literal `generic`
        /// to list only policies without a provider scope, or a
        /// provider name (e.g. `slack`) to list only per-provider
        /// policies for that provider.
        #[arg(long)]
        provider: Option<String>,
        /// Filter by principal (caller) scope. Pass the literal
        /// `any` to list only policies without a principal scope,
        /// or a caller id to list only policies scoped to that
        /// caller.
        #[arg(long)]
        principal: Option<String>,
    },
    /// Get a quota policy by ID.
    Get {
        /// Quota policy ID.
        id: String,
    },
    /// Create a quota policy. Use either --data for a raw JSON
    /// payload, or the individual --field flags for an ergonomic
    /// inline form.
    Create {
        /// JSON data (string or @file path). When set, all other
        /// --field flags are ignored.
        #[arg(long, conflicts_with_all = [
            "namespace", "tenant", "provider", "principal", "per_principal",
            "max_actions", "window", "overage_behavior", "description", "label",
        ])]
        data: Option<String>,
        /// Namespace scope.
        #[arg(long, required_unless_present = "data")]
        namespace: Option<String>,
        /// Tenant scope.
        #[arg(long, required_unless_present = "data")]
        tenant: Option<String>,
        /// Provider scope. Omit for tenant-wide.
        #[arg(long)]
        provider: Option<String>,
        /// Principal (caller id) scope. Omit to apply to every
        /// caller. Mutually exclusive with --per-principal.
        #[arg(long, conflicts_with = "per_principal")]
        principal: Option<String>,
        /// Maintain a separate counter per authenticated caller.
        #[arg(long)]
        per_principal: bool,
        /// Maximum actions allowed in the window.
        #[arg(long, required_unless_present = "data")]
        max_actions: Option<u64>,
        /// Time window: `hourly` | `daily` | `weekly` | `monthly` | integer seconds.
        #[arg(long, required_unless_present = "data")]
        window: Option<String>,
        /// Overage behavior: `block` | `warn` | `degrade:PROVIDER` | `notify:TARGET`.
        #[arg(long, required_unless_present = "data")]
        overage_behavior: Option<String>,
        /// Optional human-readable description.
        #[arg(long)]
        description: Option<String>,
        /// Label in `key=value` form. Pass multiple times for multiple labels.
        #[arg(long, value_parser = parse_kv)]
        label: Vec<(String, String)>,
    },
    /// Update a quota policy. Use either --data for a raw JSON
    /// payload, or the individual --field flags for an ergonomic
    /// inline form.
    Update {
        /// Quota policy ID.
        id: String,
        /// JSON data (string or @file path). When set, all other
        /// --field flags are ignored.
        #[arg(long, conflicts_with_all = [
            "max_actions", "window", "overage_behavior", "enabled",
            "per_principal", "description", "label",
        ])]
        data: Option<String>,
        /// Updated maximum actions.
        #[arg(long)]
        max_actions: Option<u64>,
        /// Updated time window.
        #[arg(long)]
        window: Option<String>,
        /// Updated overage behavior.
        #[arg(long)]
        overage_behavior: Option<String>,
        /// Updated enabled state.
        #[arg(long)]
        enabled: Option<bool>,
        /// Updated per-principal flag.
        #[arg(long)]
        per_principal: Option<bool>,
        /// Updated description.
        #[arg(long)]
        description: Option<String>,
        /// Replacement label set, each in `key=value` form. Pass
        /// multiple times to set multiple labels; passing none
        /// leaves the existing label set unchanged.
        #[arg(long, value_parser = parse_kv)]
        label: Vec<(String, String)>,
    },
    /// Delete a quota policy.
    Delete {
        /// Quota policy ID.
        id: String,
        /// Namespace.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Tenant.
        #[arg(long)]
        tenant: String,
    },
    /// Get quota usage.
    Usage {
        /// Quota policy ID.
        id: String,
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

/// Parse a `key=value` pair for `--label`. Rejects empty keys.
fn parse_kv(input: &str) -> Result<(String, String), String> {
    let (k, v) = input
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got {input:?}"))?;
    if k.is_empty() {
        return Err("label key must not be empty".into());
    }
    Ok((k.to_string(), v.to_string()))
}

fn labels_vec_to_map(labels: Vec<(String, String)>) -> Option<HashMap<String, String>> {
    if labels.is_empty() {
        None
    } else {
        Some(labels.into_iter().collect())
    }
}

pub async fn run(ops: &OpsClient, args: &QuotasArgs, format: &OutputFormat) -> anyhow::Result<()> {
    match &args.command {
        QuotasCommand::List {
            namespace,
            tenant,
            provider,
            principal,
        } => {
            run_list(
                ops,
                namespace.as_ref(),
                tenant.as_ref(),
                provider.as_ref(),
                principal.as_ref(),
                format,
            )
            .await
        }
        QuotasCommand::Get { id } => run_get(ops, id, format).await,
        QuotasCommand::Create {
            data,
            namespace,
            tenant,
            provider,
            principal,
            per_principal,
            max_actions,
            window,
            overage_behavior,
            description,
            label,
        } => {
            let req = if let Some(raw) = data.as_ref() {
                let value = parse_json_data(raw)?;
                serde_json::from_value::<CreateQuotaRequest>(value)?
            } else {
                CreateQuotaRequest {
                    namespace: namespace.clone().unwrap_or_default(),
                    tenant: tenant.clone().unwrap_or_default(),
                    provider: provider.clone(),
                    principal: principal.clone(),
                    per_principal: *per_principal,
                    max_actions: max_actions.unwrap_or_default(),
                    window: window.clone().unwrap_or_default(),
                    overage_behavior: overage_behavior.clone().unwrap_or_default(),
                    description: description.clone(),
                    labels: labels_vec_to_map(label.clone()),
                }
            };
            run_create(ops, &req, format).await
        }
        QuotasCommand::Update {
            id,
            data,
            max_actions,
            window,
            overage_behavior,
            enabled,
            per_principal,
            description,
            label,
        } => {
            let req = if let Some(raw) = data.as_ref() {
                let value = parse_json_data(raw)?;
                serde_json::from_value::<UpdateQuotaRequest>(value)?
            } else {
                // The Rust client's UpdateQuotaRequest insists on
                // namespace/tenant for legacy key-lookup reasons,
                // but the v1 PATCH route ignores them — pass empty
                // strings so the field is present on the wire and
                // the server does its own lookup by id.
                UpdateQuotaRequest {
                    namespace: String::new(),
                    tenant: String::new(),
                    max_actions: *max_actions,
                    window: window.clone(),
                    overage_behavior: overage_behavior.clone(),
                    description: description.clone(),
                    enabled: *enabled,
                    per_principal: *per_principal,
                    labels: labels_vec_to_map(label.clone()),
                }
            };
            run_update(ops, id, &req, format).await
        }
        QuotasCommand::Delete {
            id,
            namespace,
            tenant,
        } => {
            ops.delete_quota(id, namespace, tenant).await?;
            info!(id = %id, "Quota deleted");
            Ok(())
        }
        QuotasCommand::Usage { id } => run_usage(ops, id, format).await,
    }
}

async fn run_list(
    ops: &OpsClient,
    namespace: Option<&String>,
    tenant: Option<&String>,
    provider: Option<&String>,
    principal: Option<&String>,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops
        .list_quotas(
            namespace.map(String::as_str),
            tenant.map(String::as_str),
            provider.map(String::as_str),
            principal.map(String::as_str),
        )
        .await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(count = resp.count, "Quotas");
            for q in &resp.quotas {
                let enabled = if q.enabled { "ON " } else { "OFF" };
                let provider_scope = q.provider.as_deref().unwrap_or("*");
                let principal_scope = if q.per_principal {
                    "*per-caller".to_string()
                } else {
                    q.principal.as_deref().unwrap_or("*").to_string()
                };
                info!(
                    enabled = %enabled,
                    id = %&q.id[..8.min(q.id.len())],
                    namespace = %q.namespace,
                    tenant = %q.tenant,
                    provider = %provider_scope,
                    principal = %principal_scope,
                    max_actions = q.max_actions,
                    window = %q.window,
                    overage_behavior = %q.overage_behavior,
                    "Quota"
                );
            }
        }
    }
    Ok(())
}

async fn run_get(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_quota(id).await?;
    match resp {
        Some(q) => match format {
            OutputFormat::Json => {
                info!("{}", serde_json::to_string_pretty(&q)?);
            }
            OutputFormat::Text => {
                info!(id = %q.id, "Quota details");
                info!(namespace = %q.namespace, "  Namespace");
                info!(tenant = %q.tenant, "  Tenant");
                info!(
                    provider = %q.provider.as_deref().unwrap_or("* (generic)"),
                    "  Provider"
                );
                let principal_display = if q.per_principal {
                    "* (per caller bucket)".to_string()
                } else {
                    q.principal
                        .clone()
                        .unwrap_or_else(|| "* (any caller)".to_string())
                };
                info!(
                    principal = %principal_display,
                    "  Principal"
                );
                info!(max_actions = q.max_actions, window = %q.window, "  Max");
                info!(overage_behavior = %q.overage_behavior, "  Behavior");
                info!(enabled = q.enabled, "  Enabled");
            }
        },
        None => {
            warn!(id = %id, "Quota not found");
        }
    }
    Ok(())
}

async fn run_create(
    ops: &OpsClient,
    req: &CreateQuotaRequest,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.create_quota(req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Created quota");
        }
    }
    Ok(())
}

async fn run_update(
    ops: &OpsClient,
    id: &str,
    req: &UpdateQuotaRequest,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let resp = ops.update_quota(id, req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(id = %resp.id, "Updated quota");
        }
    }
    Ok(())
}

async fn run_usage(ops: &OpsClient, id: &str, format: &OutputFormat) -> anyhow::Result<()> {
    let resp = ops.get_quota_usage(id).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(
                used = resp.used,
                limit = resp.limit,
                remaining = resp.remaining,
                window = %resp.window,
                resets_at = %resp.resets_at,
                overage_behavior = %resp.overage_behavior,
                "Quota usage"
            );
        }
    }
    Ok(())
}
