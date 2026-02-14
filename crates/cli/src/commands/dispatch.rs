use acteon_ops::{DispatchOptions, OpsClient};
use clap::Args;
use std::collections::HashMap;

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
    /// Metadata labels (key=value).
    #[arg(long, value_parser = parse_key_val)]
    pub metadata: Vec<(String, String)>,
    /// Dry-run mode.
    #[arg(long)]
    pub dry_run: bool,
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
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

    let options = DispatchOptions {
        metadata: args.metadata.iter().cloned().collect::<HashMap<_, _>>(),
        dry_run: args.dry_run,
    };

    let outcome = ops
        .dispatch(
            args.namespace.clone(),
            args.tenant.clone(),
            args.provider.clone(),
            args.action_type.clone(),
            payload,
            options,
        )
        .await?;

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
