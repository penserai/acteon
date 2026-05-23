use acteon_ops::OpsClient;
use acteon_ops::acteon_client::{
    CreateSilenceRequest, ListSilencesQuery, MatchOp, SilenceMatcher, UpdateSilenceRequest,
};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use tracing::{info, warn};

use crate::OutputFormat;

#[derive(Args, Debug)]
pub struct SilencesArgs {
    #[command(subcommand)]
    pub command: SilencesCommand,
}

#[derive(Subcommand, Debug)]
pub enum SilencesCommand {
    /// Create a new silence.
    ///
    /// Matchers are parsed as `key=value` (`Equal`), `key!=value` (`NotEqual`),
    /// `key=~pattern` (`Regex`), or `key!~pattern` (`NotRegex`). Repeat
    /// `--matcher` to combine matchers with AND semantics.
    Create {
        /// Namespace this silence applies to.
        #[arg(long)]
        namespace: String,
        /// Tenant this silence applies to. Hierarchical matching applies —
        /// a silence on `acme` also covers `acme.us-east`.
        #[arg(long)]
        tenant: String,
        /// Label matcher in `key=value`, `key!=value`, `key=~regex`, or
        /// `key!~regex` form. Repeat for multiple matchers (AND).
        #[arg(long = "matcher", required = true)]
        matchers: Vec<String>,
        /// Explicit start time as RFC 3339. Defaults to now. Useful for
        /// scheduling a future maintenance window.
        #[arg(long)]
        starts_at: Option<DateTime<Utc>>,
        /// Duration in hours from `starts_at`. Mutually exclusive with
        /// `--ends-at`.
        #[arg(long, conflicts_with = "ends_at")]
        hours: Option<u64>,
        /// Explicit end time as RFC 3339.
        #[arg(long)]
        ends_at: Option<DateTime<Utc>>,
        /// Human-readable comment explaining why the silence exists.
        #[arg(long, default_value = "")]
        comment: String,
    },
    /// List silences.
    List {
        /// Filter by namespace.
        #[arg(long)]
        namespace: Option<String>,
        /// Filter by tenant.
        #[arg(long)]
        tenant: Option<String>,
        /// Include expired silences.
        #[arg(long)]
        include_expired: bool,
    },
    /// Get a silence by ID.
    Get {
        /// Silence ID.
        id: String,
    },
    /// Extend the end time or edit the comment of a silence.
    ///
    /// **Matchers are immutable** — to change which actions a silence
    /// covers, expire the silence with `acteon silences expire` and
    /// create a new one with the desired matcher set. This prevents
    /// race conditions where an active silence's shape changes
    /// mid-window.
    Update {
        /// Silence ID.
        id: String,
        /// New end time as RFC 3339.
        #[arg(long)]
        ends_at: Option<DateTime<Utc>>,
        /// New comment.
        #[arg(long)]
        comment: Option<String>,
    },
    /// Expire a silence immediately.
    Expire {
        /// Silence ID.
        id: String,
    },
}

pub async fn run(
    ops: &OpsClient,
    args: &SilencesArgs,
    format: &OutputFormat,
) -> anyhow::Result<()> {
    match &args.command {
        SilencesCommand::Create {
            namespace,
            tenant,
            matchers,
            starts_at,
            hours,
            ends_at,
            comment,
        } => {
            create_silence(
                ops, format, namespace, tenant, matchers, *starts_at, *hours, *ends_at, comment,
            )
            .await
        }
        SilencesCommand::List {
            namespace,
            tenant,
            include_expired,
        } => {
            list_silences(
                ops,
                format,
                namespace.as_ref(),
                tenant.as_ref(),
                *include_expired,
            )
            .await
        }
        SilencesCommand::Get { id } => get_silence(ops, format, id).await,
        SilencesCommand::Update {
            id,
            ends_at,
            comment,
        } => update_silence(ops, format, id, *ends_at, comment.as_ref()).await,
        SilencesCommand::Expire { id } => {
            ops.delete_silence(id).await?;
            info!(id = %id, "Silence expired");
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_silence(
    ops: &OpsClient,
    format: &OutputFormat,
    namespace: &str,
    tenant: &str,
    matchers: &[String],
    starts_at: Option<DateTime<Utc>>,
    hours: Option<u64>,
    ends_at: Option<DateTime<Utc>>,
    comment: &str,
) -> anyhow::Result<()> {
    let parsed: Vec<SilenceMatcher> = matchers
        .iter()
        .map(|s| parse_matcher(s))
        .collect::<Result<_, _>>()?;

    let duration_seconds = hours.map(|h| h.saturating_mul(3600));

    let req = CreateSilenceRequest {
        namespace: namespace.to_owned(),
        tenant: tenant.to_owned(),
        matchers: parsed,
        starts_at,
        ends_at,
        duration_seconds,
        comment: comment.to_owned(),
    };

    let silence = ops.create_silence(&req).await?;

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&silence)?);
        }
        OutputFormat::Text => {
            info!(id = %silence.id, "Silence created");
            info!(
                ends_at = %silence.ends_at.to_rfc3339(),
                matchers = silence.matchers.len(),
                "Active until"
            );
        }
    }
    Ok(())
}

async fn list_silences(
    ops: &OpsClient,
    format: &OutputFormat,
    namespace: Option<&String>,
    tenant: Option<&String>,
    include_expired: bool,
) -> anyhow::Result<()> {
    let query = ListSilencesQuery {
        namespace: namespace.cloned(),
        tenant: tenant.cloned(),
        include_expired,
    };
    let resp = ops.list_silences(&query).await?;

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            info!(count = resp.count, "Silences");
            for s in &resp.silences {
                let state = if s.active { "ACTIVE " } else { "EXPIRED" };
                let matchers_display = s
                    .matchers
                    .iter()
                    .map(SilenceMatcher::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                info!(
                    state = %state,
                    id = %s.id,
                    namespace = %s.namespace,
                    tenant = %s.tenant,
                    ends_at = %s.ends_at.to_rfc3339(),
                    matchers = %matchers_display,
                    comment = %s.comment,
                    "  Silence"
                );
            }
        }
    }
    Ok(())
}

async fn get_silence(ops: &OpsClient, format: &OutputFormat, id: &str) -> anyhow::Result<()> {
    let Some(s) = ops.get_silence(id).await? else {
        warn!(id = %id, "Silence not found");
        std::process::exit(1);
    };

    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&s)?);
        }
        OutputFormat::Text => {
            info!(id = %s.id, namespace = %s.namespace, tenant = %s.tenant, "Silence");
            info!(
                starts_at = %s.starts_at.to_rfc3339(),
                ends_at = %s.ends_at.to_rfc3339(),
                active = s.active,
                created_by = %s.created_by,
                comment = %s.comment,
                "Details"
            );
            for m in &s.matchers {
                info!(matcher = %m, "  Matcher");
            }
        }
    }
    Ok(())
}

async fn update_silence(
    ops: &OpsClient,
    format: &OutputFormat,
    id: &str,
    ends_at: Option<DateTime<Utc>>,
    comment: Option<&String>,
) -> anyhow::Result<()> {
    let req = UpdateSilenceRequest {
        ends_at,
        comment: comment.cloned(),
    };
    let updated = ops.update_silence(id, &req).await?;
    match format {
        OutputFormat::Json => {
            info!("{}", serde_json::to_string_pretty(&updated)?);
        }
        OutputFormat::Text => {
            info!(
                id = %updated.id,
                ends_at = %updated.ends_at.to_rfc3339(),
                comment = %updated.comment,
                "Silence updated"
            );
        }
    }
    Ok(())
}

/// Parse a matcher string in one of the four forms:
/// - `key=value` → `Equal`
/// - `key!=value` → `NotEqual`
/// - `key=~pattern` → `Regex`
/// - `key!~pattern` → `NotRegex`
///
/// Matchers must contain the operator. The key and value cannot be empty.
pub fn parse_matcher(s: &str) -> anyhow::Result<SilenceMatcher> {
    // Check longer operators first to avoid `=` matching before `=~`.
    let (name, value, op) = if let Some((k, v)) = s.split_once("!~") {
        (k, v, MatchOp::NotRegex)
    } else if let Some((k, v)) = s.split_once("=~") {
        (k, v, MatchOp::Regex)
    } else if let Some((k, v)) = s.split_once("!=") {
        (k, v, MatchOp::NotEqual)
    } else if let Some((k, v)) = s.split_once('=') {
        (k, v, MatchOp::Equal)
    } else {
        anyhow::bail!(
            "invalid matcher '{s}': expected key=value, key!=value, key=~pattern, or key!~pattern"
        );
    };

    if name.is_empty() {
        anyhow::bail!("matcher '{s}' has empty key");
    }

    SilenceMatcher::new(name, value, op).map_err(|e| anyhow::anyhow!("matcher '{s}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_equal_matcher() {
        let m = parse_matcher("severity=warning").unwrap();
        assert_eq!(m.name, "severity");
        assert_eq!(m.value, "warning");
        assert_eq!(m.op, MatchOp::Equal);
    }

    #[test]
    fn parse_not_equal_matcher() {
        let m = parse_matcher("severity!=info").unwrap();
        assert_eq!(m.name, "severity");
        assert_eq!(m.value, "info");
        assert_eq!(m.op, MatchOp::NotEqual);
    }

    #[test]
    fn parse_regex_matcher() {
        let m = parse_matcher("severity=~warn.*").unwrap();
        assert_eq!(m.name, "severity");
        assert_eq!(m.value, "warn.*");
        assert_eq!(m.op, MatchOp::Regex);
    }

    #[test]
    fn parse_not_regex_matcher() {
        let m = parse_matcher("severity!~debug|trace").unwrap();
        assert_eq!(m.name, "severity");
        assert_eq!(m.value, "debug|trace");
        assert_eq!(m.op, MatchOp::NotRegex);
    }

    #[test]
    fn parse_rejects_bare_string() {
        assert!(parse_matcher("invalid").is_err());
    }

    #[test]
    fn parse_rejects_empty_key() {
        assert!(parse_matcher("=value").is_err());
    }

    #[test]
    fn operator_precedence_prefers_not_regex_over_not_equal() {
        // `!~` must match before `!=`; otherwise `severity!~warn` would be
        // parsed as key=`severity` value=`~warn` op=NotEqual.
        let m = parse_matcher("severity!~warn").unwrap();
        assert_eq!(m.op, MatchOp::NotRegex);
        assert_eq!(m.value, "warn");
    }

    #[test]
    fn operator_precedence_prefers_regex_over_equal() {
        let m = parse_matcher("severity=~warn").unwrap();
        assert_eq!(m.op, MatchOp::Regex);
        assert_eq!(m.value, "warn");
    }
}
