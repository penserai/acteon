//! Bus agent — identity + capabilities + heartbeat for an AI-agent
//! participant on the bus (Phase 4).
//!
//! Agents share one inbox topic per `(namespace, tenant)` — named
//! `{namespace}.{tenant}.agents-inbox` by default — rather than owning
//! a topic each. Messages to a specific agent are produced with
//! `key = agent_id`, so Kafka's key-based partitioning gives each
//! agent a stable partition and per-agent FIFO ordering without any
//! subscription gymnastics.
//!
//! Status is **derived** from `last_heartbeat_at + heartbeat_ttl_ms`
//! on every read rather than via a background reaper. That keeps
//! Phase 4 free of new timers; the trade-off is that "dead" agent
//! records stay in state until an operator explicitly deletes them.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default inbox topic suffix. Combined with `{namespace}.{tenant}.` to
/// form the Kafka topic name all agents in that tenant share.
pub const DEFAULT_AGENT_INBOX_SUFFIX: &str = "agents-inbox";

/// Default heartbeat TTL: an agent that hasn't pinged in 60s is
/// reported as `Idle`; twice that and it's `Dead`. Chosen to be large
/// enough that a long GC pause or brief network blip doesn't flip an
/// otherwise healthy agent.
pub const DEFAULT_HEARTBEAT_TTL_MS: i64 = 60_000;

/// Liveness status, computed on read from the last heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum AgentStatus {
    /// Heartbeat within the TTL window — agent is live.
    Online,
    /// Heartbeat is stale but not long enough to declare dead (within
    /// `2 * heartbeat_ttl_ms` of now). Operators often want to route
    /// around idle agents without erasing them.
    Idle,
    /// Heartbeat older than `2 * heartbeat_ttl_ms`. Treat as unreachable.
    Dead,
    /// Never received a heartbeat since registration. Distinguishes
    /// fresh registrations from actually-failed agents.
    Unknown,
}

/// A bus-resident agent identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Agent {
    /// Stable identifier (e.g. `"planner-01"`). Used as the Kafka
    /// partition key when messages are sent to the inbox topic.
    pub agent_id: String,
    /// Namespace the agent belongs to.
    pub namespace: String,
    /// Tenant that owns the agent.
    pub tenant: String,
    /// Human-readable display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Capabilities the agent advertises (e.g. `["tool.calendar",
    /// "ocr", "web-search"]`). Discoverable via the `capability=`
    /// list filter.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Kafka topic the agent reads its inbox from. Defaults to
    /// `{namespace}.{tenant}.agents-inbox` when unset.
    #[serde(default)]
    pub inbox_topic: Option<String>,
    /// Heartbeat TTL in milliseconds. Within `ttl` = `Online`; within
    /// `2*ttl` = `Idle`; beyond = `Dead`.
    #[serde(default = "default_heartbeat_ttl_ms")]
    pub heartbeat_ttl_ms: i64,
    /// Last heartbeat the server has seen. `None` until the first
    /// heartbeat arrives.
    #[serde(default)]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    /// Free-form operator labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Registration time.
    pub created_at: DateTime<Utc>,
    /// Last time the agent record was mutated (register / update /
    /// heartbeat).
    pub updated_at: DateTime<Utc>,
}

fn default_heartbeat_ttl_ms() -> i64 {
    DEFAULT_HEARTBEAT_TTL_MS
}

impl Agent {
    /// Construct an agent with sensible defaults.
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            agent_id: agent_id.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            display_name: None,
            capabilities: Vec::new(),
            inbox_topic: None,
            heartbeat_ttl_ms: default_heartbeat_ttl_ms(),
            last_heartbeat_at: None,
            labels: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Canonical Kafka topic name the agent reads from. Falls back to
    /// the default `{namespace}.{tenant}.agents-inbox` when
    /// `inbox_topic` is unset.
    #[must_use]
    pub fn effective_inbox_topic(&self) -> String {
        self.inbox_topic.clone().unwrap_or_else(|| {
            format!(
                "{}.{}.{}",
                self.namespace, self.tenant, DEFAULT_AGENT_INBOX_SUFFIX
            )
        })
    }

    /// Stable state-store id. The `agent_id` alone is unique within a
    /// `(namespace, tenant)` and is already included in the `StateKey`'s
    /// higher-level scope.
    #[must_use]
    pub fn id(&self) -> String {
        self.agent_id.clone()
    }

    /// Derive the liveness status from `last_heartbeat_at` + TTL as of
    /// `now`.
    #[must_use]
    pub fn status_at(&self, now: DateTime<Utc>) -> AgentStatus {
        let Some(last) = self.last_heartbeat_at else {
            return AgentStatus::Unknown;
        };
        let age_ms = (now - last).num_milliseconds();
        if age_ms < 0 {
            // Clock skew — treat as fresh.
            return AgentStatus::Online;
        }
        if age_ms <= self.heartbeat_ttl_ms {
            AgentStatus::Online
        } else if age_ms <= self.heartbeat_ttl_ms.saturating_mul(2) {
            AgentStatus::Idle
        } else {
            AgentStatus::Dead
        }
    }

    /// Convenience — [`Self::status_at`] with `Utc::now()`.
    #[must_use]
    pub fn status(&self) -> AgentStatus {
        self.status_at(Utc::now())
    }

    /// Validate the agent's identity fields.
    pub fn validate(&self) -> Result<(), AgentValidationError> {
        Self::validate_id(&self.agent_id)?;
        Self::validate_fragment(&self.namespace)?;
        Self::validate_fragment(&self.tenant)?;
        if self.heartbeat_ttl_ms < 1_000 {
            return Err(AgentValidationError::HeartbeatTtlTooShort(
                self.heartbeat_ttl_ms,
            ));
        }
        for cap in &self.capabilities {
            Self::validate_capability(cap)?;
        }
        Ok(())
    }

    /// Agent IDs allow a slightly richer alphabet than topic fragments
    /// (dots are allowed so hierarchical names like `"planner.main"`
    /// work), but still no slashes or whitespace — the id goes into
    /// both state keys and URL paths.
    pub fn validate_id(s: &str) -> Result<(), AgentValidationError> {
        if s.is_empty() {
            return Err(AgentValidationError::EmptyId);
        }
        if s.len() > 120 {
            return Err(AgentValidationError::IdTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(AgentValidationError::InvalidIdChar(s.to_string()));
        }
        Ok(())
    }

    /// Namespace/tenant rules — reuses the topic fragment alphabet so
    /// the generated inbox topic name is always valid.
    pub fn validate_fragment(s: &str) -> Result<(), AgentValidationError> {
        if s.is_empty() {
            return Err(AgentValidationError::EmptyFragment);
        }
        if s.len() > 80 {
            return Err(AgentValidationError::FragmentTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AgentValidationError::InvalidFragmentChar(s.to_string()));
        }
        Ok(())
    }

    /// Capability tokens are dotted names like `"tool.calendar"` —
    /// same alphabet as agent IDs.
    pub fn validate_capability(s: &str) -> Result<(), AgentValidationError> {
        if s.is_empty() {
            return Err(AgentValidationError::EmptyCapability);
        }
        if s.len() > 120 {
            return Err(AgentValidationError::CapabilityTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(AgentValidationError::InvalidCapabilityChar(s.to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentValidationError {
    #[error("agent_id must not be empty")]
    EmptyId,
    #[error("agent_id exceeds 120 characters")]
    IdTooLong,
    #[error("agent_id '{0}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidIdChar(String),
    #[error("namespace/tenant fragment must not be empty")]
    EmptyFragment,
    #[error("namespace/tenant fragment exceeds 80 characters")]
    FragmentTooLong,
    #[error("namespace/tenant fragment '{0}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidFragmentChar(String),
    #[error("capability token must not be empty")]
    EmptyCapability,
    #[error("capability token exceeds 120 characters")]
    CapabilityTooLong,
    #[error("capability token '{0}' contains characters outside [a-zA-Z0-9._-]")]
    InvalidCapabilityChar(String),
    #[error("heartbeat_ttl_ms must be >= 1000 (got {0})")]
    HeartbeatTtlTooShort(i64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn effective_inbox_topic_defaults_when_unset() {
        let a = Agent::new("planner-1", "agents", "demo");
        assert_eq!(a.effective_inbox_topic(), "agents.demo.agents-inbox");
    }

    #[test]
    fn effective_inbox_topic_honors_override() {
        let mut a = Agent::new("planner-1", "agents", "demo");
        a.inbox_topic = Some("agents.demo.planners".to_string());
        assert_eq!(a.effective_inbox_topic(), "agents.demo.planners");
    }

    #[test]
    fn status_unknown_before_first_heartbeat() {
        let a = Agent::new("p", "ns", "t");
        assert_eq!(a.status(), AgentStatus::Unknown);
    }

    #[test]
    fn status_online_within_ttl() {
        let mut a = Agent::new("p", "ns", "t");
        let now = Utc::now();
        a.last_heartbeat_at = Some(now - Duration::milliseconds(5_000));
        // default ttl 60s → 5s < 60s → Online
        assert_eq!(a.status_at(now), AgentStatus::Online);
    }

    #[test]
    fn status_idle_after_single_ttl() {
        let mut a = Agent::new("p", "ns", "t");
        let now = Utc::now();
        a.last_heartbeat_at = Some(now - Duration::milliseconds(70_000));
        // 70s > 60s (online) but < 120s (dead) → Idle
        assert_eq!(a.status_at(now), AgentStatus::Idle);
    }

    #[test]
    fn status_dead_after_double_ttl() {
        let mut a = Agent::new("p", "ns", "t");
        let now = Utc::now();
        a.last_heartbeat_at = Some(now - Duration::milliseconds(300_000));
        assert_eq!(a.status_at(now), AgentStatus::Dead);
    }

    #[test]
    fn validate_rejects_short_ttl() {
        let mut a = Agent::new("p", "ns", "t");
        a.heartbeat_ttl_ms = 500;
        assert_eq!(
            a.validate(),
            Err(AgentValidationError::HeartbeatTtlTooShort(500))
        );
    }

    #[test]
    fn validate_rejects_slash_in_id() {
        let a = Agent::new("a/b", "ns", "t");
        assert!(matches!(
            a.validate(),
            Err(AgentValidationError::InvalidIdChar(_))
        ));
    }

    #[test]
    fn validate_accepts_dotted_id() {
        let a = Agent::new("planner.main", "ns", "t");
        a.validate().unwrap();
    }

    #[test]
    fn validate_rejects_invalid_capability() {
        let mut a = Agent::new("p", "ns", "t");
        a.capabilities.push("bad cap".to_string());
        assert!(matches!(
            a.validate(),
            Err(AgentValidationError::InvalidCapabilityChar(_))
        ));
    }

    #[test]
    fn roundtrip_serde() {
        let mut a = Agent::new("p", "ns", "t");
        a.capabilities.push("ocr".to_string());
        a.labels.insert("owner".into(), "ml-team".into());
        let j = serde_json::to_string(&a).unwrap();
        let back: Agent = serde_json::from_str(&j).unwrap();
        assert_eq!(back.agent_id, a.agent_id);
        assert_eq!(back.capabilities, a.capabilities);
        assert_eq!(back.labels, a.labels);
    }
}
