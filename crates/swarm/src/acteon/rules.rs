use acteon_rules::RuleFrontend;
use serde_json::{Value, json};
use std::io::Write as _;

use crate::config::{SafetyConfig, SwarmConfig};
use crate::error::SwarmError;

/// Generate parser-compatible policies scoped to exactly one namespace and tenant.
/// Command patterns are defense in depth; agents still require an OS execution sandbox.
pub fn generate_safety_rules(namespace: &str, tenant: &str, safety: &SafetyConfig) -> String {
    let scope = md5_short(&format!("{namespace}\0{tenant}"));
    let mut rules = Vec::new();
    let mut add = |name: &str, priority: i32, predicates: Vec<Value>, action: Value| {
        let mut all = vec![
            json!({"field":"action.namespace", "eq":namespace}),
            json!({"field":"action.tenant", "eq":tenant}),
        ];
        all.extend(predicates);
        rules.push(json!({"name":format!("swarm-{scope}-{name}"), "priority":priority, "condition":{"all":all}, "action":action}));
    };
    let command = |pattern: &str| {
        vec![
            json!({"field":"action.action_type", "eq":"execute_command"}),
            json!({"field":"action.payload.command", "matches":pattern}),
        ]
    };
    add(
        "block-destructive-commands",
        1,
        command(
            r"(?i)(\brm\s+-[a-z]*r[a-z]*f|\brm\s+-[a-z]*f[a-z]*r|\bmkfs\b|\bdd\s+if=|\bshutdown\b|\breboot\b)",
        ),
        json!({"type":"suppress"}),
    );
    for field in ["file_path", "path"] {
        add(
            &format!("block-credential-{field}"),
            1,
            vec![
                json!({"field":"action.action_type", "eq":"write_file"}),
                json!({"field":format!("action.payload.{field}"), "matches":r"(\.env($|[./])|\.ssh/|credentials\.json|\.aws/|\bsecrets\.)"}),
            ],
            json!({"type":"suppress"}),
        );
    }
    add(
        "block-network-shell",
        1,
        command(r"(?i)\b(curl|wget|nc|netcat)\b"),
        json!({"type":"suppress"}),
    );
    for pattern in &safety.blocked_commands {
        add(
            &format!("custom-block-{}", md5_short(pattern)),
            1,
            command(pattern),
            json!({"type":"suppress"}),
        );
    }
    for (name, pattern) in [
        ("approve-git-push", r"\bgit\s+([^;&|]*\s)?push\b"),
        (
            "approve-package-install",
            r"\b(pip|pip3|npm|cargo|apt|brew)\s+(install|add)\b",
        ),
    ] {
        add(
            name,
            3,
            command(pattern),
            json!({"type":"request_approval", "notify_provider":safety.approval_notify_provider, "timeout_seconds":safety.approval_timeout_seconds}),
        );
    }
    add(
        "throttle-commands",
        5,
        vec![json!({"field":"action.action_type", "eq":"execute_command"})],
        json!({"type":"throttle", "max_count":12,"window_seconds":60}),
    );
    add(
        "dedup-cross-agent-writes",
        6,
        vec![json!({"field":"action.action_type", "eq":"write_file"})],
        json!({"type":"deduplicate", "ttl_seconds":120}),
    );
    add(
        "allow-web-access",
        15,
        vec![json!({"field":"action.action_type", "eq":"web_access"})],
        json!({"type":"allow"}),
    );
    add("default-deny", 100, vec![], json!({"type":"suppress"}));
    serde_yaml_ng::to_string(&json!({"rules":rules})).expect("JSON rule values serialize to YAML")
}

/// Install the run policy into a shared gateway rule directory, then verify reload.
/// Missing configuration or a rejected policy prevents agents from starting.
pub async fn install_safety_rules(config: &SwarmConfig, tenant: &str) -> Result<(), SwarmError> {
    let directory = config.safety.rules_directory.as_ref().ok_or_else(|| SwarmError::Config(
        "safety.rules_directory must name the gateway's shared rules directory before starting agents".into()
    ))?;
    let yaml = generate_safety_rules(&config.acteon.namespace, tenant, &config.safety);
    let parsed = acteon_rules_yaml::YamlFrontend.parse(&yaml).map_err(|e| {
        SwarmError::Config(format!("generated safety policy failed validation: {e}"))
    })?;
    let filename = format!(
        "swarm-{}.yaml",
        md5_short(&format!("{}\0{tenant}", config.acteon.namespace))
    );
    tokio::fs::create_dir_all(directory).await?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(yaml.as_bytes())?;
    temporary
        .persist(directory.join(filename))
        .map_err(|e| SwarmError::Io(e.error))?;
    let mut builder = acteon_client::ActeonClientBuilder::new(&config.acteon.endpoint);
    if let Some(key) = &config.acteon.api_key {
        builder = builder.api_key(key);
    }
    let client = builder
        .build()
        .map_err(|e| SwarmError::Acteon(e.to_string()))?;
    let providers = client
        .list_provider_health()
        .await
        .map_err(|e| SwarmError::Acteon(e.to_string()))?;
    if !providers.providers.iter().any(|provider| {
        provider.provider == config.safety.approval_notify_provider && provider.healthy
    }) {
        return Err(SwarmError::Config(
            "approval notification provider must be registered and healthy".into(),
        ));
    }
    let reload = client
        .reload_rules()
        .await
        .map_err(|e| SwarmError::Acteon(e.to_string()))?;
    if !reload.errors.is_empty() {
        return Err(SwarmError::Config(format!(
            "gateway rejected policy reload: {:?}",
            reload.errors
        )));
    }
    let loaded = client
        .list_rules()
        .await
        .map_err(|e| SwarmError::Acteon(e.to_string()))?;
    if parsed.iter().any(|rule| {
        !loaded
            .iter()
            .any(|remote| remote.name == rule.name && remote.enabled)
    }) {
        return Err(SwarmError::Config(
            "gateway did not load all run safety rules; verify shared directory configuration"
                .into(),
        ));
    }
    // A globally higher-priority allow rule must not silently override safety.
    let probe = acteon_core::Action::new(
        config.acteon.namespace.as_str(),
        tenant,
        "claude-code",
        "execute_command",
        json!({"command":"rm -rf /tmp/acteon-policy-probe"}),
    );
    let outcome = client
        .dispatch_dry_run(&probe)
        .await
        .map_err(|e| SwarmError::Acteon(e.to_string()))?;
    if !matches!(outcome, acteon_core::ActionOutcome::DryRun { verdict, matched_rule: Some(ref name), .. }
        if verdict == "suppress" && parsed.iter().any(|rule| &rule.name == name))
    {
        return Err(SwarmError::Config(
            "installed safety policy is shadowed or ineffective in the gateway".into(),
        ));
    }

    Ok(())
}

fn md5_short(input: &str) -> String {
    use md5::{Digest, Md5};
    format!("{:x}", Md5::digest(input.as_bytes()))[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_policy_round_trips_through_real_parser() {
        let safety = SafetyConfig {
            blocked_commands: vec![r#"sudo\s+.*\"quoted\""#.into()],
            ..Default::default()
        };
        let yaml = generate_safety_rules("namespace: \"quoted\"", "tenant\nwith newline", &safety);
        let rules = acteon_rules_yaml::YamlFrontend.parse(&yaml).unwrap();
        assert_eq!(rules.len(), 11);
        assert!(rules.iter().any(|r| r.name.ends_with("default-deny")));
        assert!(
            rules
                .iter()
                .any(|r| matches!(r.action, acteon_rules::RuleAction::RequestApproval { .. }))
        );
    }

    #[test]
    fn different_tenants_cannot_replace_each_others_rules() {
        let a = acteon_rules_yaml::YamlFrontend
            .parse(&generate_safety_rules("ns", "a", &SafetyConfig::default()))
            .unwrap();
        let b = acteon_rules_yaml::YamlFrontend
            .parse(&generate_safety_rules("ns", "b", &SafetyConfig::default()))
            .unwrap();
        assert!(
            a.iter()
                .all(|rule| b.iter().all(|other| rule.name != other.name))
        );
    }
}
