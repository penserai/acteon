use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A data retention policy for a tenant.
///
/// Controls how long audit records, completed chains, and resolved events
/// are kept before automatic cleanup by the background reaper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RetentionPolicy {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Namespace this policy applies to.
    pub namespace: String,
    /// Tenant this policy applies to.
    pub tenant: String,
    /// Whether this retention policy is currently active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Override for the global audit TTL (seconds).
    ///
    /// When set, audit records for this tenant expire after this many seconds
    /// instead of the gateway-wide default.
    #[serde(default)]
    pub audit_ttl_seconds: Option<u64>,
    /// TTL for completed/failed/cancelled chain state records (seconds).
    ///
    /// Completed chains older than this are deleted by the background reaper.
    #[serde(default)]
    pub state_ttl_seconds: Option<u64>,
    /// TTL for resolved event state records (seconds).
    ///
    /// Resolved events older than this are deleted by the background reaper.
    #[serde(default)]
    pub event_ttl_seconds: Option<u64>,
    /// When `true`, audit records for this tenant never expire (compliance hold).
    ///
    /// Overrides `audit_ttl_seconds` and the global audit TTL. Useful for
    /// SOC2/HIPAA compliance where audit records must be preserved indefinitely.
    #[serde(default)]
    pub compliance_hold: bool,
    /// When this policy was created.
    pub created_at: DateTime<Utc>,
    /// When this policy was last updated.
    pub updated_at: DateTime<Utc>,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary key-value labels for filtering and organization.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_policy_serde_roundtrip() {
        let policy = RetentionPolicy {
            id: "ret-001".into(),
            namespace: "notifications".into(),
            tenant: "tenant-1".into(),
            enabled: true,
            audit_ttl_seconds: Some(86_400),
            state_ttl_seconds: Some(604_800),
            event_ttl_seconds: Some(259_200),
            compliance_hold: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: Some("30-day audit, 7-day state".into()),
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "ret-001");
        assert_eq!(back.audit_ttl_seconds, Some(86_400));
        assert_eq!(back.state_ttl_seconds, Some(604_800));
        assert_eq!(back.event_ttl_seconds, Some(259_200));
        assert!(!back.compliance_hold);
        assert!(back.enabled);
    }

    #[test]
    fn retention_policy_deserializes_with_defaults() {
        let json = r#"{
            "id": "ret-002",
            "namespace": "ns",
            "tenant": "t",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;

        let policy: RetentionPolicy = serde_json::from_str(json).unwrap();
        assert!(policy.enabled);
        assert!(policy.audit_ttl_seconds.is_none());
        assert!(policy.state_ttl_seconds.is_none());
        assert!(policy.event_ttl_seconds.is_none());
        assert!(!policy.compliance_hold);
        assert!(policy.description.is_none());
        assert!(policy.labels.is_empty());
    }

    #[test]
    fn retention_policy_compliance_hold() {
        let policy = RetentionPolicy {
            id: "ret-003".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            enabled: true,
            audit_ttl_seconds: Some(86_400),
            state_ttl_seconds: None,
            event_ttl_seconds: None,
            compliance_hold: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert!(back.compliance_hold);
    }

    #[test]
    fn retention_policy_with_labels() {
        let mut labels = HashMap::new();
        labels.insert("tier".into(), "enterprise".into());
        labels.insert("region".into(), "us-east-1".into());

        let policy = RetentionPolicy {
            id: "ret-004".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            enabled: true,
            audit_ttl_seconds: None,
            state_ttl_seconds: None,
            event_ttl_seconds: None,
            compliance_hold: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels,
        };

        let json = serde_json::to_string_pretty(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.labels.get("tier"), Some(&"enterprise".to_string()));
        assert_eq!(back.labels.get("region"), Some(&"us-east-1".to_string()));
    }

    #[test]
    fn retention_policy_disabled() {
        let policy = RetentionPolicy {
            id: "ret-dis".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            enabled: false,
            audit_ttl_seconds: Some(3600),
            state_ttl_seconds: None,
            event_ttl_seconds: None,
            compliance_hold: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert!(!back.enabled);
    }
}
