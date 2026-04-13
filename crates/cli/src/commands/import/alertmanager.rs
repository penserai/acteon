//! Import a Prometheus Alertmanager `alertmanager.yml` config into
//! Acteon's TOML provider config + YAML rules.
//!
//! This is a **migration tool** — it reads the Alertmanager routing
//! tree and emits the closest Acteon equivalent so operators can
//! review, tweak, and switch over without manually rewriting their
//! routing topology.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use clap::Args;
use serde::Deserialize;

// =========================================================================
// CLI args
// =========================================================================

/// Import routing config from a Prometheus Alertmanager config file.
#[derive(Args, Debug)]
pub struct AlertmanagerImportArgs {
    /// Path to the alertmanager.yml file.
    #[arg(short, long)]
    pub config: PathBuf,

    /// Directory to write providers.toml + rules.yaml.
    /// Defaults to the current directory.
    #[arg(short, long, default_value = ".")]
    pub output_dir: PathBuf,

    /// Print generated config to stdout instead of writing files.
    #[arg(long)]
    pub dry_run: bool,

    /// Default namespace for generated rules.
    #[arg(long, default_value = "default")]
    pub default_namespace: String,
}

// =========================================================================
// Alertmanager YAML model (subset of the full spec)
// =========================================================================

#[derive(Debug, Deserialize)]
struct AlertmanagerConfig {
    #[serde(default)]
    global: Option<GlobalConfig>,
    route: Route,
    #[serde(default)]
    receivers: Vec<Receiver>,
    #[serde(default)]
    inhibit_rules: Option<Vec<InhibitRule>>,
    /// Alertmanager `time_intervals:` (preferred name).
    #[serde(default)]
    time_intervals: Option<Vec<AmTimeIntervalDef>>,
    /// Alertmanager legacy `mute_time_intervals:` — same shape as
    /// `time_intervals`, kept here so old configs still parse.
    #[serde(default)]
    mute_time_intervals: Option<Vec<AmTimeIntervalDef>>,
}

#[allow(dead_code)] // Fields used for deserialization completeness
#[derive(Debug, Default, Deserialize)]
struct GlobalConfig {
    smtp_smarthost: Option<String>,
    smtp_from: Option<String>,
    smtp_require_tls: Option<bool>,
    slack_api_url: Option<String>,
    opsgenie_api_url: Option<String>,
    pagerduty_url: Option<String>,
}

#[allow(dead_code)] // Fields used for deserialization completeness
#[derive(Debug, Deserialize)]
struct Route {
    receiver: Option<String>,
    #[serde(default)]
    group_by: Option<Vec<String>>,
    group_wait: Option<String>,
    group_interval: Option<String>,
    repeat_interval: Option<String>,
    #[serde(rename = "match", default)]
    match_labels: Option<HashMap<String, String>>,
    #[serde(default)]
    match_re: Option<HashMap<String, String>>,
    #[serde(default)]
    routes: Option<Vec<Route>>,
    #[serde(rename = "continue", default)]
    continue_routing: Option<bool>,
    /// Names of time intervals during which this route is muted.
    #[serde(default)]
    mute_time_intervals: Option<Vec<String>>,
    /// Names of time intervals during which this route is active.
    #[serde(default)]
    active_time_intervals: Option<Vec<String>>,
}

// =========================================================================
// Time interval model — Alertmanager's `time_intervals:` schema
// =========================================================================

#[derive(Debug, Deserialize)]
struct AmTimeIntervalDef {
    name: String,
    #[serde(default)]
    time_intervals: Vec<AmTimeIntervalEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct AmTimeIntervalEntry {
    #[serde(default)]
    times: Vec<AmTimeRange>,
    #[serde(default)]
    weekdays: Vec<String>,
    #[serde(default)]
    days_of_month: Vec<String>,
    #[serde(default)]
    months: Vec<String>,
    #[serde(default)]
    years: Vec<String>,
    #[serde(default)]
    location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AmTimeRange {
    start_time: String,
    end_time: String,
}

#[derive(Debug, Deserialize)]
struct Receiver {
    name: String,
    #[serde(default)]
    slack_configs: Option<Vec<SlackConfig>>,
    #[serde(default)]
    pagerduty_configs: Option<Vec<PagerdutyConfig>>,
    #[serde(default)]
    opsgenie_configs: Option<Vec<OpsgenieConfig>>,
    #[serde(default)]
    webhook_configs: Option<Vec<WebhookConfig>>,
    #[serde(default)]
    email_configs: Option<Vec<EmailConfig>>,
    #[serde(default)]
    victorops_configs: Option<Vec<VictoropsConfig>>,
    #[serde(default)]
    pushover_configs: Option<Vec<PushoverConfig>>,
}

#[derive(Debug, Deserialize)]
struct SlackConfig {
    channel: Option<String>,
    api_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PagerdutyConfig {
    routing_key: Option<String>,
    service_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpsgenieConfig {
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebhookConfig {
    url: Option<String>,
}

#[allow(dead_code)] // Fields used for deserialization completeness
#[derive(Debug, Deserialize)]
struct EmailConfig {
    to: Option<String>,
    from: Option<String>,
    smarthost: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VictoropsConfig {
    routing_key: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PushoverConfig {
    user_key: Option<String>,
    token: Option<String>,
}

#[allow(dead_code)] // Fields used for deserialization completeness
#[derive(Debug, Deserialize)]
struct InhibitRule {
    #[serde(default)]
    source_match: Option<HashMap<String, String>>,
    #[serde(default)]
    source_match_re: Option<HashMap<String, String>>,
    #[serde(default)]
    target_match: Option<HashMap<String, String>>,
    #[serde(default)]
    target_match_re: Option<HashMap<String, String>>,
    #[serde(default)]
    equal: Option<Vec<String>>,
}

// =========================================================================
// Conversion
// =========================================================================

/// A generated Acteon provider entry.
struct ProviderEntry {
    name: String,
    provider_type: String,
    comment: Option<String>,
    fields: Vec<(String, String)>,
}

/// A generated Acteon rule entry.
struct RuleEntry {
    name: String,
    priority: u32,
    conditions: Vec<ConditionEntry>,
    action_type: String,
    action_fields: Vec<(String, String)>,
    /// Names of time intervals during which this rule is muted.
    mute_time_intervals: Vec<String>,
    /// Names of time intervals during which this rule is active.
    active_time_intervals: Vec<String>,
}

/// A generated Acteon time interval entry, ready for YAML rendering.
struct TimeIntervalEntry {
    name: String,
    /// One `TimeRange` per element. Each inner Vec is the raw key/value
    /// lines for that range (already YAML-escaped).
    ranges: Vec<Vec<(String, String)>>,
    location: Option<String>,
}

enum ConditionEntry {
    Eq(String, String),
    Matches(String, String),
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn parse_duration_to_seconds(s: &str) -> u64 {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('s') {
        rest.parse().unwrap_or(30)
    } else if let Some(rest) = s.strip_suffix('m') {
        rest.parse::<u64>().unwrap_or(5) * 60
    } else if let Some(rest) = s.strip_suffix('h') {
        rest.parse::<u64>().unwrap_or(1) * 3600
    } else if let Some(rest) = s.strip_suffix('d') {
        rest.parse::<u64>().unwrap_or(1) * 86400
    } else {
        s.parse().unwrap_or(30)
    }
}

#[allow(clippy::too_many_lines)]
fn convert_receivers(receivers: &[Receiver], global: &GlobalConfig) -> Vec<ProviderEntry> {
    let mut providers = Vec::new();

    for recv in receivers {
        // Slack
        if let Some(ref cfgs) = recv.slack_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                let mut fields = Vec::new();
                if let Some(ref ch) = cfg.channel {
                    fields.push(("default_channel".into(), ch.clone()));
                }
                let url = cfg.api_url.as_deref().or(global.slack_api_url.as_deref());
                providers.push(ProviderEntry {
                    name,
                    provider_type: if url.is_some() {
                        "webhook".into()
                    } else {
                        "log".into()
                    },
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (slack).{}",
                        recv.name,
                        if url.is_none() {
                            " TODO: set webhook_url or switch type to 'slack'."
                        } else {
                            ""
                        }
                    )),
                    fields: if let Some(u) = url {
                        vec![("webhook_url".into(), u.to_owned())]
                    } else {
                        fields
                    },
                });
            }
        }

        // PagerDuty
        if let Some(ref cfgs) = recv.pagerduty_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                let key = cfg.routing_key.as_deref().or(cfg.service_key.as_deref());
                providers.push(ProviderEntry {
                    name,
                    provider_type: "pagerduty".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (pagerduty).{}",
                        recv.name,
                        if key.is_none() {
                            " TODO: set routing key."
                        } else {
                            ""
                        }
                    )),
                    fields: if let Some(k) = key {
                        vec![("routing_key".into(), k.to_owned())]
                    } else {
                        vec![("routing_key".into(), "TODO".into())]
                    },
                });
            }
        }

        // OpsGenie
        if let Some(ref cfgs) = recv.opsgenie_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                providers.push(ProviderEntry {
                    name,
                    provider_type: "opsgenie".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (opsgenie). Requires --features opsgenie.",
                        recv.name,
                    )),
                    fields: vec![(
                        "opsgenie.api_key".into(),
                        cfg.api_key.clone().unwrap_or_else(|| "TODO".into()),
                    )],
                });
            }
        }

        // Webhook
        if let Some(ref cfgs) = recv.webhook_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                providers.push(ProviderEntry {
                    name,
                    provider_type: "webhook".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (webhook).",
                        recv.name,
                    )),
                    fields: vec![(
                        "webhook_url".into(),
                        cfg.url.clone().unwrap_or_else(|| "TODO".into()),
                    )],
                });
            }
        }

        // Email
        if let Some(ref cfgs) = recv.email_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                let host = cfg
                    .smarthost
                    .as_deref()
                    .or(global.smtp_smarthost.as_deref())
                    .unwrap_or("localhost:25");
                let parts: Vec<&str> = host.splitn(2, ':').collect();
                providers.push(ProviderEntry {
                    name,
                    provider_type: "email".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (email).",
                        recv.name,
                    )),
                    fields: vec![
                        ("email_backend".into(), "smtp".into()),
                        ("smtp_host".into(), parts[0].to_owned()),
                        (
                            "smtp_port".into(),
                            parts.get(1).unwrap_or(&"25").to_string(),
                        ),
                        (
                            "from_address".into(),
                            cfg.from
                                .as_deref()
                                .or(global.smtp_from.as_deref())
                                .unwrap_or("acteon@localhost")
                                .to_owned(),
                        ),
                    ],
                });
            }
        }

        // VictorOps
        if let Some(ref cfgs) = recv.victorops_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                providers.push(ProviderEntry {
                    name: name.clone(),
                    provider_type: "victorops".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (victorops). Requires --features victorops.",
                        recv.name,
                    )),
                    fields: vec![
                        (
                            "victorops.api_key".into(),
                            cfg.api_key.clone().unwrap_or_else(|| "TODO".into()),
                        ),
                        (
                            format!("victorops.routes.{name}"),
                            cfg.routing_key.clone().unwrap_or_else(|| "TODO".into()),
                        ),
                    ],
                });
            }
        }

        // Pushover
        if let Some(ref cfgs) = recv.pushover_configs {
            for (i, cfg) in cfgs.iter().enumerate() {
                let suffix = if cfgs.len() > 1 {
                    format!("-{}", i + 1)
                } else {
                    String::new()
                };
                let name = format!("{}{suffix}", sanitize_name(&recv.name));
                providers.push(ProviderEntry {
                    name: name.clone(),
                    provider_type: "pushover".into(),
                    comment: Some(format!(
                        "Imported from Alertmanager receiver '{}' (pushover). Requires --features pushover.",
                        recv.name,
                    )),
                    fields: vec![
                        (
                            "pushover.app_token".into(),
                            cfg.token.clone().unwrap_or_else(|| "TODO".into()),
                        ),
                        (
                            format!("pushover.recipients.{name}"),
                            cfg.user_key.clone().unwrap_or_else(|| "TODO".into()),
                        ),
                    ],
                });
            }
        }

        // If no configs matched, emit a log placeholder
        let has_any = recv.slack_configs.as_ref().is_some_and(|v| !v.is_empty())
            || recv
                .pagerduty_configs
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || recv
                .opsgenie_configs
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || recv.webhook_configs.as_ref().is_some_and(|v| !v.is_empty())
            || recv.email_configs.as_ref().is_some_and(|v| !v.is_empty())
            || recv
                .victorops_configs
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || recv
                .pushover_configs
                .as_ref()
                .is_some_and(|v| !v.is_empty());
        if !has_any {
            providers.push(ProviderEntry {
                name: sanitize_name(&recv.name),
                provider_type: "log".into(),
                comment: Some(format!(
                    "Imported from Alertmanager receiver '{}' (no supported configs found). Replace with real provider.",
                    recv.name,
                )),
                fields: vec![],
            });
        }
    }

    providers
}

fn convert_routes(route: &Route, priority: &mut u32, rules: &mut Vec<RuleEntry>) {
    if let Some(ref children) = route.routes {
        for child in children {
            let mut conditions = Vec::new();

            if let Some(ref m) = child.match_labels {
                for (k, v) in m {
                    conditions.push(ConditionEntry::Eq(
                        format!("action.metadata.{k}"),
                        v.clone(),
                    ));
                }
            }
            if let Some(ref m) = child.match_re {
                for (k, v) in m {
                    conditions.push(ConditionEntry::Matches(
                        format!("action.metadata.{k}"),
                        v.clone(),
                    ));
                }
            }

            if conditions.is_empty() && child.receiver.is_none() {
                // Skip empty intermediate nodes
                convert_routes(child, priority, rules);
                continue;
            }

            let receiver = child
                .receiver
                .as_deref()
                .or(route.receiver.as_deref())
                .unwrap_or("log");

            // Determine action: group if group_by is set, otherwise reroute
            let has_grouping = child.group_by.as_ref().is_some_and(|g| !g.is_empty());

            let (action_type, action_fields) = if has_grouping {
                let group_by = child.group_by.as_ref().unwrap();
                let gw = child
                    .group_wait
                    .as_deref()
                    .or(route.group_wait.as_deref())
                    .map_or(30, parse_duration_to_seconds);
                let gi = child
                    .group_interval
                    .as_deref()
                    .or(route.group_interval.as_deref())
                    .map_or(300, parse_duration_to_seconds);
                let ri = child
                    .repeat_interval
                    .as_deref()
                    .or(route.repeat_interval.as_deref());
                let mut fields = vec![
                    ("group_by".into(), format!("{group_by:?}")),
                    ("group_wait_seconds".into(), gw.to_string()),
                    ("group_interval_seconds".into(), gi.to_string()),
                ];
                if let Some(r) = ri {
                    fields.push((
                        "repeat_interval_seconds".into(),
                        parse_duration_to_seconds(r).to_string(),
                    ));
                }
                ("group".into(), fields)
            } else {
                (
                    "reroute".into(),
                    vec![("target_provider".into(), sanitize_name(receiver))],
                )
            };

            let label_hint: Vec<&str> = conditions
                .iter()
                .map(|c| match c {
                    ConditionEntry::Eq(_, v) | ConditionEntry::Matches(_, v) => v.as_str(),
                })
                .collect();
            let name_suffix = if label_hint.is_empty() {
                sanitize_name(receiver)
            } else {
                sanitize_name(&label_hint.join("-"))
            };

            // Time interval references propagate from parent down: if
            // the child route doesn't declare any, inherit the parent's.
            let mute_intervals = child
                .mute_time_intervals
                .clone()
                .or_else(|| route.mute_time_intervals.clone())
                .unwrap_or_default();
            let active_intervals = child
                .active_time_intervals
                .clone()
                .or_else(|| route.active_time_intervals.clone())
                .unwrap_or_default();

            rules.push(RuleEntry {
                name: format!("imported-{name_suffix}-{priority}"),
                priority: *priority,
                conditions,
                action_type,
                action_fields,
                mute_time_intervals: mute_intervals,
                active_time_intervals: active_intervals,
            });
            *priority += 1;

            // Recurse into children
            convert_routes(child, priority, rules);
        }
    }
}

fn convert_inhibit_rules(
    inhibit_rules: &[InhibitRule],
    priority: &mut u32,
    rules: &mut Vec<RuleEntry>,
) {
    for (i, rule) in inhibit_rules.iter().enumerate() {
        let mut conditions = Vec::new();

        if let Some(ref m) = rule.target_match {
            for (k, v) in m {
                conditions.push(ConditionEntry::Eq(
                    format!("action.metadata.{k}"),
                    v.clone(),
                ));
            }
        }
        if let Some(ref m) = rule.target_match_re {
            for (k, v) in m {
                conditions.push(ConditionEntry::Matches(
                    format!("action.metadata.{k}"),
                    v.clone(),
                ));
            }
        }

        if conditions.is_empty() {
            continue;
        }

        rules.push(RuleEntry {
            name: format!("imported-inhibit-{}", i + 1),
            priority: *priority,
            conditions,
            action_type: "suppress".into(),
            action_fields: vec![],
            mute_time_intervals: Vec::new(),
            active_time_intervals: Vec::new(),
        });
        *priority += 1;
    }
}

/// Convert Alertmanager `time_intervals` → Acteon time interval entries.
///
/// Alertmanager's range model uses string forms (`"monday:friday"`,
/// `"1:31"`, `"-1"`, `"jan:dec"`). Acteon stores them as numeric
/// `(start, end)` ranges. This function does the textual→numeric
/// translation. Ranges that fail to parse are skipped with a warning
/// so importing a malformed entry never crashes the CLI.
fn convert_time_intervals(defs: &[AmTimeIntervalDef]) -> Vec<TimeIntervalEntry> {
    let mut out = Vec::new();
    for def in defs {
        let mut ranges: Vec<Vec<(String, String)>> = Vec::new();
        let mut location: Option<String> = None;
        for entry in &def.time_intervals {
            if location.is_none() {
                location.clone_from(&entry.location);
            }
            let mut fields: Vec<(String, String)> = Vec::new();

            // times
            if !entry.times.is_empty() {
                let mut yaml = String::from("\n");
                for t in &entry.times {
                    let _ = writeln!(
                        yaml,
                        "          - start: \"{}\"\n            end: \"{}\"",
                        t.start_time, t.end_time
                    );
                }
                fields.push(("times".into(), yaml.trim_end().to_owned()));
            }

            // weekdays — Alertmanager: "monday:friday" or "saturday"
            if !entry.weekdays.is_empty() {
                let mut yaml = String::from("\n");
                for w in &entry.weekdays {
                    if let Some((s, e)) = parse_weekday_range(w) {
                        let _ = writeln!(yaml, "          - start: {s}\n            end: {e}");
                    } else {
                        eprintln!(
                            "warning: time_intervals[{}]: unrecognized weekday {w:?}, skipping",
                            def.name
                        );
                    }
                }
                fields.push(("weekdays".into(), yaml.trim_end().to_owned()));
            }

            // days_of_month — "1:31", "-1", "15"
            if !entry.days_of_month.is_empty() {
                let mut yaml = String::from("\n");
                for d in &entry.days_of_month {
                    if let Some((s, e)) = parse_int_range(d) {
                        let _ = writeln!(yaml, "          - start: {s}\n            end: {e}");
                    } else {
                        eprintln!(
                            "warning: time_intervals[{}]: bad days_of_month {d:?}, skipping",
                            def.name
                        );
                    }
                }
                fields.push(("days_of_month".into(), yaml.trim_end().to_owned()));
            }

            // months — "january:december", "april"
            if !entry.months.is_empty() {
                let mut yaml = String::from("\n");
                for m in &entry.months {
                    if let Some((s, e)) = parse_month_range(m) {
                        let _ = writeln!(yaml, "          - start: {s}\n            end: {e}");
                    } else {
                        eprintln!(
                            "warning: time_intervals[{}]: bad month {m:?}, skipping",
                            def.name
                        );
                    }
                }
                fields.push(("months".into(), yaml.trim_end().to_owned()));
            }

            // years — "2025:2030", "2026"
            if !entry.years.is_empty() {
                let mut yaml = String::from("\n");
                for y in &entry.years {
                    if let Some((s, e)) = parse_int_range(y) {
                        let _ = writeln!(yaml, "          - start: {s}\n            end: {e}");
                    } else {
                        eprintln!(
                            "warning: time_intervals[{}]: bad year {y:?}, skipping",
                            def.name
                        );
                    }
                }
                fields.push(("years".into(), yaml.trim_end().to_owned()));
            }

            ranges.push(fields);
        }

        out.push(TimeIntervalEntry {
            name: sanitize_name(&def.name),
            ranges,
            location,
        });
    }
    out
}

/// Parse a colon-delimited range like `"1:31"` or a single value like `"-1"`.
fn parse_int_range(s: &str) -> Option<(i32, i32)> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(':') {
        let a = a.trim().parse().ok()?;
        let b = b.trim().parse().ok()?;
        Some((a, b))
    } else {
        let v: i32 = s.parse().ok()?;
        Some((v, v))
    }
}

/// Convert a weekday name to its 1..=7 (Mon..Sun) index.
fn weekday_index(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "monday" | "mon" => Some(1),
        "tuesday" | "tue" => Some(2),
        "wednesday" | "wed" => Some(3),
        "thursday" | "thu" => Some(4),
        "friday" | "fri" => Some(5),
        "saturday" | "sat" => Some(6),
        "sunday" | "sun" => Some(7),
        _ => None,
    }
}

fn parse_weekday_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(':') {
        Some((weekday_index(a)?, weekday_index(b)?))
    } else {
        let v = weekday_index(s)?;
        Some((v, v))
    }
}

fn month_index(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "january" | "jan" => Some(1),
        "february" | "feb" => Some(2),
        "march" | "mar" => Some(3),
        "april" | "apr" => Some(4),
        "may" => Some(5),
        "june" | "jun" => Some(6),
        "july" | "jul" => Some(7),
        "august" | "aug" => Some(8),
        "september" | "sep" => Some(9),
        "october" | "oct" => Some(10),
        "november" | "nov" => Some(11),
        "december" | "dec" => Some(12),
        // Alertmanager also accepts numeric months.
        other => other.parse().ok().filter(|&n: &u32| (1..=12).contains(&n)),
    }
}

fn parse_month_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if let Some((a, b)) = s.split_once(':') {
        Some((month_index(a)?, month_index(b)?))
    } else {
        let v = month_index(s)?;
        Some((v, v))
    }
}

// =========================================================================
// Output rendering
// =========================================================================

fn render_providers_toml(providers: &[ProviderEntry]) -> String {
    let mut out = String::from(
        "# Auto-generated by: acteon import alertmanager\n\
         # Review and replace placeholder values (TODO) before use.\n\n",
    );

    for p in providers {
        if let Some(ref c) = p.comment {
            let _ = writeln!(out, "# {c}");
        }
        out.push_str("[[providers]]\n");
        let _ = writeln!(out, "name = \"{}\"", p.name);
        let _ = writeln!(out, "type = \"{}\"", p.provider_type);
        for (k, v) in &p.fields {
            let _ = writeln!(out, "{k} = \"{v}\"");
        }
        out.push('\n');
    }

    out
}

fn render_rules_yaml(rules: &[RuleEntry]) -> String {
    let mut out = String::from(
        "# Auto-generated by: acteon import alertmanager\n\
         # Review rule priorities and conditions before use.\n\
         rules:\n",
    );

    for r in rules {
        let _ = writeln!(out, "  - name: {}", r.name);
        let _ = writeln!(out, "    priority: {}", r.priority);

        // Condition
        if r.conditions.len() == 1 {
            match &r.conditions[0] {
                ConditionEntry::Eq(field, val) => {
                    out.push_str("    condition:\n");
                    let _ = writeln!(out, "      field: {field}");
                    let _ = writeln!(out, "      eq: \"{val}\"");
                }
                ConditionEntry::Matches(field, val) => {
                    out.push_str("    condition:\n");
                    let _ = writeln!(out, "      field: {field}");
                    let _ = writeln!(out, "      matches: \"{val}\"");
                }
            }
        } else if r.conditions.len() > 1 {
            out.push_str("    condition:\n");
            out.push_str("      all:\n");
            for c in &r.conditions {
                match c {
                    ConditionEntry::Eq(field, val) => {
                        let _ = writeln!(out, "        - field: {field}");
                        let _ = writeln!(out, "          eq: \"{val}\"");
                    }
                    ConditionEntry::Matches(field, val) => {
                        let _ = writeln!(out, "        - field: {field}");
                        let _ = writeln!(out, "          matches: \"{val}\"");
                    }
                }
            }
        }

        // Action
        out.push_str("    action:\n");
        let _ = writeln!(out, "      type: {}", r.action_type);
        for (k, v) in &r.action_fields {
            if k == "group_by" {
                // Emit as YAML list
                let items: Vec<&str> = v
                    .trim_matches(|c| c == '[' || c == ']' || c == '"')
                    .split(", ")
                    .map(|s| s.trim_matches('"'))
                    .collect();
                out.push_str("      group_by:\n");
                for item in items {
                    let _ = writeln!(out, "        - {item}");
                }
            } else {
                let _ = writeln!(out, "      {k}: {v}");
            }
        }

        // Time interval references
        if !r.mute_time_intervals.is_empty() {
            out.push_str("    mute_time_intervals:\n");
            for n in &r.mute_time_intervals {
                let _ = writeln!(out, "      - {n}");
            }
        }
        if !r.active_time_intervals.is_empty() {
            out.push_str("    active_time_intervals:\n");
            for n in &r.active_time_intervals {
                let _ = writeln!(out, "      - {n}");
            }
        }
    }

    out
}

/// Render imported time intervals as a YAML document. The format mirrors
/// the create-time-interval API request body so operators can drop the
/// file into `acteon-cli time-intervals create -f ...` (or the future
/// bulk-load path).
fn render_time_intervals_yaml(intervals: &[TimeIntervalEntry], namespace: &str) -> String {
    let mut out = String::from(
        "# Auto-generated by: acteon import alertmanager\n\
         # Each entry maps to a `POST /v1/time-intervals` request.\n\
         # Review the resolved namespace and tenant before applying.\n\
         time_intervals:\n",
    );
    for ti in intervals {
        let _ = writeln!(out, "  - name: {}", ti.name);
        let _ = writeln!(out, "    namespace: {namespace}");
        out.push_str("    tenant: default\n");
        if let Some(ref loc) = ti.location {
            let _ = writeln!(out, "    location: \"{loc}\"");
        }
        out.push_str("    time_ranges:\n");
        for range in &ti.ranges {
            out.push_str("      - ");
            let mut first = true;
            for (k, v) in range {
                if first {
                    let _ = writeln!(out, "{k}:{v}");
                    first = false;
                } else {
                    let _ = writeln!(out, "        {k}:{v}");
                }
            }
            if first {
                // No predicates at all (rare): emit an empty range so
                // the YAML stays parseable as a list of objects.
                out.push_str("{}\n");
            }
        }
    }
    out
}

// =========================================================================
// Entry point
// =========================================================================

pub fn run(args: &AlertmanagerImportArgs) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(&args.config)?;
    let config: AlertmanagerConfig = serde_yaml_ng::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse alertmanager config: {e}"))?;

    // Merge `time_intervals:` and the legacy `mute_time_intervals:`
    // top-level lists. Alertmanager treats both as the same shape.
    let mut all_time_intervals: Vec<AmTimeIntervalDef> = Vec::new();
    if let Some(ti) = config.time_intervals {
        all_time_intervals.extend(ti);
    }
    if let Some(ti) = config.mute_time_intervals {
        all_time_intervals.extend(ti);
    }

    eprintln!(
        "Parsed {} receivers, {} route children, {} inhibit rules, {} time intervals",
        config.receivers.len(),
        config.route.routes.as_ref().map_or(0, Vec::len),
        config.inhibit_rules.as_ref().map_or(0, Vec::len),
        all_time_intervals.len(),
    );

    // Convert receivers → providers
    let global = config.global.unwrap_or_default();
    let providers = convert_receivers(&config.receivers, &global);
    eprintln!("Generated {} provider(s)", providers.len());

    // Convert route tree → rules
    let mut rules = Vec::new();
    let mut priority = 1u32;
    convert_routes(&config.route, &mut priority, &mut rules);

    // Convert inhibit rules → suppress rules
    if let Some(ref inhibit) = config.inhibit_rules {
        convert_inhibit_rules(inhibit, &mut priority, &mut rules);
    }
    eprintln!("Generated {} rule(s)", rules.len());

    // Convert time intervals
    let time_intervals = convert_time_intervals(&all_time_intervals);
    if !time_intervals.is_empty() {
        eprintln!("Generated {} time interval(s)", time_intervals.len());
    }

    // Render output
    let providers_toml = render_providers_toml(&providers);
    let rules_yaml = render_rules_yaml(&rules);
    let time_intervals_yaml = if time_intervals.is_empty() {
        None
    } else {
        Some(render_time_intervals_yaml(
            &time_intervals,
            &args.default_namespace,
        ))
    };

    if args.dry_run {
        println!("--- providers.toml ---");
        println!("{providers_toml}");
        println!("--- rules.yaml ---");
        println!("{rules_yaml}");
        if let Some(ref ti) = time_intervals_yaml {
            println!("--- time-intervals.yaml ---");
            println!("{ti}");
        }
    } else {
        std::fs::create_dir_all(&args.output_dir)?;
        let providers_path = args.output_dir.join("providers.toml");
        let rules_path = args.output_dir.join("rules.yaml");
        std::fs::write(&providers_path, &providers_toml)?;
        std::fs::write(&rules_path, &rules_yaml)?;
        eprintln!("Wrote {}", providers_path.display());
        eprintln!("Wrote {}", rules_path.display());
        if let Some(ref ti) = time_intervals_yaml {
            let ti_path = args.output_dir.join("time-intervals.yaml");
            std::fs::write(&ti_path, ti)?;
            eprintln!("Wrote {}", ti_path.display());
        }
    }

    Ok(())
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &str = r##"
global:
  smtp_smarthost: "mail.example.com:587"
  smtp_from: "alerts@example.com"
  slack_api_url: "https://hooks.slack.com/services/T00/B00/xxx"

route:
  receiver: default-slack
  group_by: [alertname, cluster]
  group_wait: 30s
  group_interval: 5m
  repeat_interval: 4h
  routes:
    - match:
        severity: critical
      receiver: pagerduty-critical
    - match:
        severity: warning
      receiver: slack-warnings
      group_by: [alertname]
      group_wait: 1m
    - match_re:
        service: "web-.*"
      receiver: webhook-web

receivers:
  - name: default-slack
    slack_configs:
      - channel: "#alerts"
  - name: pagerduty-critical
    pagerduty_configs:
      - routing_key: "abc123"
  - name: slack-warnings
    slack_configs:
      - channel: "#warnings"
  - name: webhook-web
    webhook_configs:
      - url: "https://hooks.example.com/web"

inhibit_rules:
  - source_match:
      severity: critical
    target_match:
      severity: warning
    equal: [alertname, cluster]
"##;

    #[test]
    fn parse_sample_config() {
        let config: AlertmanagerConfig =
            serde_yaml_ng::from_str(SAMPLE_CONFIG).expect("parse sample");
        assert_eq!(config.receivers.len(), 4);
        assert!(config.route.routes.as_ref().unwrap().len() >= 3);
        assert_eq!(config.inhibit_rules.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn convert_sample_providers() {
        let config: AlertmanagerConfig = serde_yaml_ng::from_str(SAMPLE_CONFIG).expect("parse");
        let global = config.global.unwrap_or_default();
        let providers = convert_receivers(&config.receivers, &global);
        assert!(providers.len() >= 4);

        let pd = providers.iter().find(|p| p.name == "pagerduty-critical");
        assert!(pd.is_some());
        assert_eq!(pd.unwrap().provider_type, "pagerduty");
    }

    #[test]
    fn convert_sample_routes() {
        let config: AlertmanagerConfig = serde_yaml_ng::from_str(SAMPLE_CONFIG).expect("parse");
        let mut rules = Vec::new();
        let mut priority = 1;
        convert_routes(&config.route, &mut priority, &mut rules);
        assert!(rules.len() >= 3);

        // First rule should be for severity=critical → pagerduty-critical
        let crit = &rules[0];
        assert_eq!(crit.action_type, "reroute");
        assert!(crit.conditions.iter().any(|c| matches!(
            c,
            ConditionEntry::Eq(f, v) if f.contains("severity") && v == "critical"
        )));
    }

    #[test]
    fn convert_inhibit_rules() {
        let config: AlertmanagerConfig = serde_yaml_ng::from_str(SAMPLE_CONFIG).expect("parse");
        let mut rules = Vec::new();
        let mut priority = 100;
        super::convert_inhibit_rules(
            config.inhibit_rules.as_ref().unwrap(),
            &mut priority,
            &mut rules,
        );
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action_type, "suppress");
    }

    #[test]
    fn parse_durations() {
        assert_eq!(parse_duration_to_seconds("30s"), 30);
        assert_eq!(parse_duration_to_seconds("5m"), 300);
        assert_eq!(parse_duration_to_seconds("1h"), 3600);
        assert_eq!(parse_duration_to_seconds("2d"), 172800);
    }

    const TIME_INTERVAL_CONFIG: &str = r##"
route:
  receiver: default
  routes:
    - match:
        severity: warning
      receiver: slack
      mute_time_intervals:
        - weekend-mute
      active_time_intervals:
        - business-hours

receivers:
  - name: default
    slack_configs:
      - channel: "#alerts"
  - name: slack
    slack_configs:
      - channel: "#warnings"

time_intervals:
  - name: business-hours
    time_intervals:
      - times:
          - start_time: "09:00"
            end_time: "17:00"
        weekdays:
          - "monday:friday"
        location: "America/New_York"
  - name: weekend-mute
    time_intervals:
      - weekdays:
          - saturday
          - sunday
"##;

    #[test]
    fn parses_time_intervals_top_level() {
        let config: AlertmanagerConfig =
            serde_yaml_ng::from_str(TIME_INTERVAL_CONFIG).expect("parse");
        let ti = config.time_intervals.as_ref().expect("time_intervals");
        assert_eq!(ti.len(), 2);
        assert_eq!(ti[0].name, "business-hours");
        assert_eq!(ti[0].time_intervals[0].times.len(), 1);
        assert_eq!(ti[0].time_intervals[0].weekdays, vec!["monday:friday"]);
        assert_eq!(
            ti[0].time_intervals[0].location.as_deref(),
            Some("America/New_York")
        );
    }

    #[test]
    fn route_inherits_time_interval_refs() {
        let config: AlertmanagerConfig =
            serde_yaml_ng::from_str(TIME_INTERVAL_CONFIG).expect("parse");
        let mut rules = Vec::new();
        let mut priority = 1;
        convert_routes(&config.route, &mut priority, &mut rules);
        assert!(!rules.is_empty(), "expected child route rule");
        let warn = rules
            .iter()
            .find(|r| {
                r.conditions
                    .iter()
                    .any(|c| matches!(c, ConditionEntry::Eq(_, v) if v == "warning"))
            })
            .expect("warning rule");
        assert_eq!(warn.mute_time_intervals, vec!["weekend-mute".to_owned()]);
        assert_eq!(
            warn.active_time_intervals,
            vec!["business-hours".to_owned()]
        );
    }

    #[test]
    fn convert_time_intervals_renders_yaml() {
        let config: AlertmanagerConfig =
            serde_yaml_ng::from_str(TIME_INTERVAL_CONFIG).expect("parse");
        let intervals = convert_time_intervals(config.time_intervals.as_ref().unwrap());
        assert_eq!(intervals.len(), 2);
        let yaml = render_time_intervals_yaml(&intervals, "prod");
        assert!(yaml.contains("name: business-hours"));
        assert!(yaml.contains("namespace: prod"));
        assert!(yaml.contains("location: \"America/New_York\""));
        // Weekday `monday:friday` becomes 1..5; weekend single-day becomes 6,6 / 7,7.
        assert!(yaml.contains("start: 1"));
        assert!(yaml.contains("end: 5"));
        assert!(yaml.contains("start: 6"));
        assert!(yaml.contains("start: 7"));
    }

    #[test]
    fn weekday_and_month_parsing() {
        assert_eq!(weekday_index("monday"), Some(1));
        assert_eq!(weekday_index("Sun"), Some(7));
        assert_eq!(weekday_index("foo"), None);
        assert_eq!(parse_weekday_range("monday:friday"), Some((1, 5)));
        assert_eq!(month_index("january"), Some(1));
        assert_eq!(month_index("Dec"), Some(12));
        assert_eq!(month_index("4"), Some(4));
        assert_eq!(parse_month_range("jan:apr"), Some((1, 4)));
        assert_eq!(parse_int_range("1:31"), Some((1, 31)));
        assert_eq!(parse_int_range("-1"), Some((-1, -1)));
    }

    #[test]
    fn render_produces_valid_toml_and_yaml() {
        let config: AlertmanagerConfig = serde_yaml_ng::from_str(SAMPLE_CONFIG).expect("parse");
        let global = config.global.unwrap_or_default();
        let providers = convert_receivers(&config.receivers, &global);
        let toml_out = render_providers_toml(&providers);
        assert!(toml_out.contains("[[providers]]"));
        assert!(toml_out.contains("pagerduty-critical"));

        let mut rules = Vec::new();
        let mut priority = 1;
        convert_routes(&config.route, &mut priority, &mut rules);
        let yaml_out = render_rules_yaml(&rules);
        assert!(yaml_out.contains("rules:"));
        assert!(yaml_out.contains("reroute"));
    }
}
