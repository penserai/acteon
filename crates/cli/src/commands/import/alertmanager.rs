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

            rules.push(RuleEntry {
                name: format!("imported-{name_suffix}-{priority}"),
                priority: *priority,
                conditions,
                action_type,
                action_fields,
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
        });
        *priority += 1;
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

    eprintln!(
        "Parsed {} receivers, {} route children, {} inhibit rules",
        config.receivers.len(),
        config.route.routes.as_ref().map_or(0, Vec::len),
        config.inhibit_rules.as_ref().map_or(0, Vec::len),
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

    // Render output
    let providers_toml = render_providers_toml(&providers);
    let rules_yaml = render_rules_yaml(&rules);

    if args.dry_run {
        println!("--- providers.toml ---");
        println!("{providers_toml}");
        println!("--- rules.yaml ---");
        println!("{rules_yaml}");
    } else {
        std::fs::create_dir_all(&args.output_dir)?;
        let providers_path = args.output_dir.join("providers.toml");
        let rules_path = args.output_dir.join("rules.yaml");
        std::fs::write(&providers_path, &providers_toml)?;
        std::fs::write(&rules_path, &rules_yaml)?;
        eprintln!("Wrote {}", providers_path.display());
        eprintln!("Wrote {}", rules_path.display());
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
