use std::fmt::Write as _;

use crate::config::SafetyConfig;

/// Generate default safety rules YAML for a swarm run.
///
/// These rules are deployed to Acteon before agents start and enforce:
/// - Hard blocks on destructive commands
/// - Approval gates for sensitive operations
/// - Per-agent and swarm-wide throttling
/// - Cross-agent file write deduplication
pub fn generate_safety_rules(namespace: &str, tenant: &str, safety: &SafetyConfig) -> String {
    let extra_blocked = build_custom_blocks(&safety.blocked_commands);
    build_base_rules(namespace, tenant, &extra_blocked, safety)
}

/// Build YAML fragments for custom blocked command patterns.
fn build_custom_blocks(blocked_commands: &[String]) -> String {
    let mut extra_blocked = String::new();
    for pattern in blocked_commands {
        let hash = md5_short(pattern);
        let _ = write!(
            extra_blocked,
            r#"
  - name: custom-block-{hash}
    priority: 1
    description: "Custom blocked pattern"
    conditions:
      action_type: execute_command
      payload:
        command:
          regex: "{pattern}"
    action: suppress
"#
        );
    }
    extra_blocked
}

/// Build the complete rules YAML from base layers and custom blocks.
fn build_base_rules(
    namespace: &str,
    tenant: &str,
    extra_blocked: &str,
    safety: &SafetyConfig,
) -> String {
    let approval_timeout = safety.approval_timeout_seconds;
    format!(
        r#"# Auto-generated safety rules for swarm run
# Namespace: {namespace}, Tenant: {tenant}

rules:
  # ── Layer 0: Hard blocks ──────────────────────────────────────────────────
  - name: block-destructive-commands
    priority: 1
    description: "Block destructive shell commands"
    conditions:
      action_type: execute_command
      payload:
        command:
          regex: "(rm\\s+-rf|mkfs|dd\\s+if=|format\\s+c:|shutdown|reboot)"
    action: suppress

  - name: block-credential-access
    priority: 1
    description: "Block access to credential files"
    conditions:
      action_type: write_file
      payload:
        file_path:
          regex: "(\\.env|\\.ssh/|credentials\\.json|\\.aws/|\\bsecrets\\.)"
    action: suppress

  - name: block-data-exfiltration
    priority: 1
    description: "Block sending data to external hosts"
    conditions:
      action_type: execute_command
      payload:
        command:
          regex: "(curl|wget|nc)\\s+.*[^localhost|127\\.0\\.0\\.1]"
    action: suppress
{extra_blocked}
  # ── Layer 1: Approval gates ───────────────────────────────────────────────
  - name: approve-git-push
    priority: 3
    description: "Require approval for git push operations"
    conditions:
      action_type: execute_command
      payload:
        command:
          regex: "git\\s+(push|force-push)"
    action:
      request_approval:
        timeout_seconds: {approval_timeout}
        message: "Agent wants to push to git remote"

  - name: approve-package-install
    priority: 3
    description: "Require approval for package installations"
    conditions:
      action_type: execute_command
      payload:
        command:
          regex: "(pip install|npm install|cargo install|apt install|brew install)"
    action:
      request_approval:
        timeout_seconds: {approval_timeout}
        message: "Agent wants to install packages"

  # ── Layer 2: Throttle and dedup ───────────────────────────────────────────
  - name: throttle-per-agent
    priority: 5
    description: "Rate-limit per-agent command execution"
    conditions:
      action_type: execute_command
    action:
      throttle:
        max_count: 12
        window_seconds: 60

  - name: dedup-cross-agent-writes
    priority: 6
    description: "Deduplicate cross-agent writes to the same file"
    conditions:
      action_type: write_file
    action:
      deduplicate:
        ttl_seconds: 120

  - name: throttle-swarm-wide
    priority: 7
    description: "Swarm-wide action rate limit"
    action:
      throttle:
        max_count: 30
        window_seconds: 60

  # ── Layer 3: Allow normal operations ──────────────────────────────────────
  - name: allow-normal-ops
    priority: 15
    description: "Allow standard coding operations"
    conditions:
      action_type:
        any_of: [execute_command, write_file, web_access]
    action: allow

  # ── Layer 4: Default deny ─────────────────────────────────────────────────
  - name: default-deny
    priority: 100
    description: "Catch-all deny for unmatched actions"
    action: suppress
"#
    )
}

fn md5_short(input: &str) -> String {
    use md5::{Digest, Md5};
    let result = Md5::digest(input.as_bytes());
    format!("{result:x}")[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SafetyConfig;

    #[test]
    fn test_generate_default_rules() {
        let rules = generate_safety_rules("swarm", "run-123", &SafetyConfig::default());
        assert!(rules.contains("block-destructive-commands"));
        assert!(rules.contains("approve-git-push"));
        assert!(rules.contains("throttle-per-agent"));
        assert!(rules.contains("dedup-cross-agent-writes"));
        assert!(rules.contains("default-deny"));
    }

    #[test]
    fn test_custom_blocked_commands() {
        let safety = SafetyConfig {
            blocked_commands: vec!["sudo.*".into()],
            ..SafetyConfig::default()
        };
        let rules = generate_safety_rules("swarm", "run-123", &safety);
        assert!(rules.contains("sudo.*"));
        assert!(rules.contains("custom-block-"));
    }
}
