use acteon_ops::OpsClient;
use acteon_ops::acteon_core::Action;
use clap::Args;

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct DispatchArgs {
    /// Namespace.
    #[arg(long, default_value = "default")]
    pub namespace: String,
    /// Tenant.
    #[arg(long)]
    pub tenant: String,
    /// Provider name.
    #[arg(long)]
    pub provider: String,
    /// Action type.
    #[arg(long, name = "type")]
    pub action_type: String,
    /// JSON payload (string or @file path).
    #[arg(long)]
    pub payload: String,
    /// Dry-run mode.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(
    ops: &OpsClient,
    args: &DispatchArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    let payload: serde_json::Value = if args.payload.starts_with('@') {
        let path = &args.payload[1..];
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::from_str(&args.payload)?
    };

    let action = Action::new(
        args.namespace.as_str(),
        args.tenant.as_str(),
        args.provider.as_str(),
        &args.action_type,
        payload,
    );

    let outcome = if args.dry_run {
        ops.client().dispatch_dry_run(&action).await?
    } else {
        ops.client().dispatch(&action).await?
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&outcome)?);
        }
        OutputFormat::Text => {
            println!("{outcome:?}");
        }
    }

    Ok(())
}
