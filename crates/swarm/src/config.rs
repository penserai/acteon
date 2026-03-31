use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::SwarmError;

/// Top-level swarm configuration, typically loaded from `swarm.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmConfig {
    /// Acteon gateway connection settings.
    #[serde(default)]
    pub acteon: ActeonConnectionConfig,
    /// `TesseraiDB` connection settings.
    #[serde(default)]
    pub tesserai: TesseraiConnectionConfig,
    /// Default values for swarm execution.
    #[serde(default)]
    pub defaults: SwarmDefaults,
    /// Safety and policy settings.
    #[serde(default)]
    pub safety: SafetyConfig,
    /// Custom role definitions (extend/override built-in roles).
    #[serde(default)]
    pub roles: Vec<AgentRoleConfig>,
    /// Adversarial swarm configuration.
    #[serde(default)]
    pub adversarial: AdversarialConfig,
    /// Eval harness configuration (fitness-driven verification).
    #[serde(default)]
    pub eval_harness: EvalHarnessConfig,
}

/// Connection settings for the Acteon gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActeonConnectionConfig {
    /// Acteon HTTP endpoint.
    #[serde(default = "default_acteon_endpoint")]
    pub endpoint: String,
    /// Optional API key for authentication.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Namespace for swarm actions.
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

impl Default for ActeonConnectionConfig {
    fn default() -> Self {
        Self {
            endpoint: default_acteon_endpoint(),
            api_key: None,
            namespace: default_namespace(),
        }
    }
}

/// Connection settings for `TesseraiDB`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TesseraiConnectionConfig {
    /// `TesseraiDB` HTTP endpoint.
    #[serde(default = "default_tesserai_endpoint")]
    pub endpoint: String,
    /// Optional API key for authentication.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Tenant ID for isolation.
    #[serde(default = "default_tesserai_tenant")]
    pub tenant_id: String,
}

impl Default for TesseraiConnectionConfig {
    fn default() -> Self {
        Self {
            endpoint: default_tesserai_endpoint(),
            api_key: None,
            tenant_id: default_tesserai_tenant(),
        }
    }
}

/// Default execution parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmDefaults {
    /// The AI engine to use for agents.
    #[serde(default)]
    pub engine: AgentEngine,
    /// Maximum concurrent agents.
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,
    /// Maximum total run duration in minutes.
    #[serde(default = "default_max_duration")]
    pub max_duration_minutes: u64,
    /// Default timeout per subtask in seconds.
    #[serde(default = "default_subtask_timeout")]
    pub subtask_timeout_seconds: u64,
    /// Maximum actions in the per-run quota.
    #[serde(default = "default_quota_max")]
    pub quota_max_actions: u64,
    /// Quota window in seconds.
    #[serde(default = "default_quota_window")]
    pub quota_window_seconds: u64,
    /// Working directory override (defaults to CWD).
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    /// Enable the AI-powered plan refiner after each task completes.
    #[serde(default = "default_true")]
    pub enable_refiner: bool,
}

/// Supported AI engines for swarm agents.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentEngine {
    #[default]
    Claude,
    Gemini,
}

impl Default for SwarmDefaults {
    fn default() -> Self {
        Self {
            engine: AgentEngine::default(),
            max_agents: default_max_agents(),
            max_duration_minutes: default_max_duration(),
            subtask_timeout_seconds: default_subtask_timeout(),
            quota_max_actions: default_quota_max(),
            quota_window_seconds: default_quota_window(),
            working_directory: None,
            enable_refiner: true,
        }
    }
}

/// Adversarial swarm configuration.
///
/// When enabled, the primary swarm's output is challenged by an adversarial
/// swarm that can optionally use a different AI engine (e.g., Claude primary
/// with Gemini adversarial). The primary swarm then re-processes to recover
/// from the adversarial feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialConfig {
    /// Enable the adversarial challenge-recovery loop.
    #[serde(default)]
    pub enabled: bool,
    /// AI engine for the adversarial swarm (defaults to the primary engine).
    #[serde(default)]
    pub engine: Option<AgentEngine>,
    /// Maximum challenge-recovery rounds.
    #[serde(default = "default_adversarial_rounds")]
    pub max_rounds: usize,
    /// Maximum concurrent adversarial agents.
    #[serde(default = "default_adversarial_agents")]
    pub max_agents: usize,
    /// Timeout for each challenge phase in seconds.
    #[serde(default = "default_challenge_timeout")]
    pub challenge_timeout_seconds: u64,
    /// Timeout for each recovery phase in seconds.
    #[serde(default = "default_recovery_timeout")]
    pub recovery_timeout_seconds: u64,
    /// Minimum severity (0.0–1.0) for a challenge to trigger recovery.
    /// Challenges below this threshold are logged but do not block completion.
    #[serde(default = "default_severity_threshold")]
    pub severity_threshold: f64,
    /// Custom adversarial role definitions.
    #[serde(default)]
    pub roles: Vec<AgentRoleConfig>,
}

impl Default for AdversarialConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            engine: None,
            max_rounds: default_adversarial_rounds(),
            max_agents: default_adversarial_agents(),
            challenge_timeout_seconds: default_challenge_timeout(),
            recovery_timeout_seconds: default_recovery_timeout(),
            severity_threshold: default_severity_threshold(),
            roles: Vec::new(),
        }
    }
}

impl AdversarialConfig {
    /// Returns the engine for the adversarial swarm, falling back to the given
    /// primary engine if none is explicitly configured.
    pub fn effective_engine(&self, primary: AgentEngine) -> AgentEngine {
        self.engine.unwrap_or(primary)
    }
}

/// Eval harness configuration — fitness-driven verification inspired by
/// Karpathy's autoresearch pattern.
///
/// When enabled, a deterministic eval command runs after the primary swarm
/// and after each adversarial recovery round. The output is parsed for score
/// signals (`SCORE:`, `PASS:`, `WARNINGS:`). The adversarial loop uses score
/// changes to gate continuation and keep/discard recovery changes via git.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalHarnessConfig {
    /// Enable the eval harness.
    #[serde(default)]
    pub enabled: bool,
    /// Shell command to run (e.g., `"cargo test && cargo clippy"`).
    #[serde(default)]
    pub command: String,
    /// Timeout for the eval command in seconds.
    #[serde(default = "default_eval_timeout")]
    pub timeout_seconds: u64,
    /// Minimum score (0.0–1.0) to consider a pass.
    #[serde(default = "default_pass_threshold")]
    pub pass_threshold: f64,
}

impl Default for EvalHarnessConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: String::new(),
            timeout_seconds: default_eval_timeout(),
            pass_threshold: default_pass_threshold(),
        }
    }
}

/// Safety and policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    /// Require human approval before executing the plan.
    #[serde(default = "default_true")]
    pub require_plan_approval: bool,
    /// Timeout for human approval in seconds.
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_seconds: u64,
    /// Additional regex patterns to block.
    #[serde(default)]
    pub blocked_commands: Vec<String>,
    /// Directory containing custom rule YAML files.
    #[serde(default)]
    pub rules_directory: Option<PathBuf>,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            require_plan_approval: true,
            approval_timeout_seconds: default_approval_timeout(),
            blocked_commands: Vec::new(),
            rules_directory: None,
        }
    }
}

/// Custom agent role definition in config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    pub name: String,
    pub description: String,
    pub system_prompt_template: String,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub can_delegate_to: Vec<String>,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
}

impl SwarmConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, SwarmError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            SwarmError::Config(format!(
                "failed to read config file {}: {e}",
                path.display()
            ))
        })?;
        toml::from_str(&content).map_err(SwarmError::Toml)
    }

    /// Create a minimal config with defaults (useful for testing).
    pub fn minimal() -> Self {
        Self {
            acteon: ActeonConnectionConfig::default(),
            tesserai: TesseraiConnectionConfig::default(),
            defaults: SwarmDefaults::default(),
            safety: SafetyConfig::default(),
            roles: Vec::new(),
            adversarial: AdversarialConfig::default(),
            eval_harness: EvalHarnessConfig::default(),
        }
    }
}

// ── Default value functions ──────────────────────────────────────────────────

fn default_acteon_endpoint() -> String {
    "http://localhost:8080".into()
}
fn default_namespace() -> String {
    "swarm".into()
}
fn default_tesserai_endpoint() -> String {
    "http://localhost:8081".into()
}
fn default_tesserai_tenant() -> String {
    "swarm-default".into()
}
fn default_max_agents() -> usize {
    5
}
fn default_max_duration() -> u64 {
    120
}
fn default_subtask_timeout() -> u64 {
    900
}
fn default_quota_max() -> u64 {
    500
}
fn default_quota_window() -> u64 {
    3600
}
fn default_true() -> bool {
    true
}
fn default_approval_timeout() -> u64 {
    600
}
fn default_max_concurrent() -> usize {
    2
}
fn default_adversarial_rounds() -> usize {
    2
}
fn default_adversarial_agents() -> usize {
    3
}
fn default_challenge_timeout() -> u64 {
    600
}
fn default_recovery_timeout() -> u64 {
    900
}
fn default_severity_threshold() -> f64 {
    0.5
}
fn default_eval_timeout() -> u64 {
    300
}
fn default_pass_threshold() -> f64 {
    0.7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let config = SwarmConfig::minimal();
        assert_eq!(config.acteon.endpoint, "http://localhost:8080");
        assert_eq!(config.tesserai.endpoint, "http://localhost:8081");
        assert_eq!(config.defaults.max_agents, 5);
        assert!(config.safety.require_plan_approval);
    }

    #[test]
    fn test_config_from_toml() {
        let toml_str = r#"
[acteon]
endpoint = "http://acteon:9090"
namespace = "test-swarm"

[tesserai]
endpoint = "http://tesserai:8082"
tenant_id = "test-tenant"

[defaults]
max_agents = 3
max_duration_minutes = 15

[safety]
require_plan_approval = false
blocked_commands = ["sudo.*"]

[[roles]]
name = "db-admin"
description = "Database administrator"
system_prompt_template = "You manage databases."
allowed_tools = ["Bash", "Read"]
"#;
        let config: SwarmConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.acteon.endpoint, "http://acteon:9090");
        assert_eq!(config.acteon.namespace, "test-swarm");
        assert_eq!(config.tesserai.tenant_id, "test-tenant");
        assert_eq!(config.defaults.max_agents, 3);
        assert!(!config.safety.require_plan_approval);
        assert_eq!(config.roles.len(), 1);
        assert_eq!(config.roles[0].name, "db-admin");
        // Adversarial defaults when not specified.
        assert!(!config.adversarial.enabled);
        assert_eq!(config.adversarial.max_rounds, 2);
    }

    #[test]
    fn test_adversarial_config_from_toml() {
        let toml_str = r#"
[adversarial]
enabled = true
engine = "gemini"
max_rounds = 3
max_agents = 4
challenge_timeout_seconds = 300
recovery_timeout_seconds = 600
severity_threshold = 0.7
"#;
        let config: SwarmConfig = toml::from_str(toml_str).unwrap();
        assert!(config.adversarial.enabled);
        assert_eq!(config.adversarial.engine, Some(AgentEngine::Gemini));
        assert_eq!(config.adversarial.max_rounds, 3);
        assert_eq!(config.adversarial.max_agents, 4);
        assert_eq!(config.adversarial.challenge_timeout_seconds, 300);
        assert_eq!(config.adversarial.recovery_timeout_seconds, 600);
        assert!((config.adversarial.severity_threshold - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_adversarial_effective_engine() {
        let mut config = AdversarialConfig::default();
        // Falls back to primary when None.
        assert_eq!(
            config.effective_engine(AgentEngine::Claude),
            AgentEngine::Claude
        );
        // Uses explicit engine when set.
        config.engine = Some(AgentEngine::Gemini);
        assert_eq!(
            config.effective_engine(AgentEngine::Claude),
            AgentEngine::Gemini
        );
    }

    #[test]
    fn test_eval_harness_config_from_toml() {
        let toml_str = r#"
[eval_harness]
enabled = true
command = "cargo test && cargo clippy"
timeout_seconds = 120
pass_threshold = 0.8
"#;
        let config: SwarmConfig = toml::from_str(toml_str).unwrap();
        assert!(config.eval_harness.enabled);
        assert_eq!(config.eval_harness.command, "cargo test && cargo clippy");
        assert_eq!(config.eval_harness.timeout_seconds, 120);
        assert!((config.eval_harness.pass_threshold - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_eval_harness_defaults() {
        let config = SwarmConfig::minimal();
        assert!(!config.eval_harness.enabled);
        assert!(config.eval_harness.command.is_empty());
        assert_eq!(config.eval_harness.timeout_seconds, 300);
        assert!((config.eval_harness.pass_threshold - 0.7).abs() < f64::EPSILON);
    }
}
