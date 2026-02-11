use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Time window over which quota usage is tracked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    /// Rolling 1-hour window.
    Hourly,
    /// Rolling 24-hour window.
    Daily,
    /// Rolling 7-day window.
    Weekly,
    /// Rolling 30-day window.
    Monthly,
    /// Custom window duration in seconds.
    Custom {
        /// Window duration in seconds.
        seconds: u64,
    },
}

impl QuotaWindow {
    /// Return the window duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> u64 {
        match self {
            Self::Hourly => 3_600,
            Self::Daily => 86_400,
            Self::Weekly => 604_800,
            Self::Monthly => 2_592_000,
            Self::Custom { seconds } => *seconds,
        }
    }

    /// Return a short label for display and state key generation.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Hourly => "hourly".to_owned(),
            Self::Daily => "daily".to_owned(),
            Self::Weekly => "weekly".to_owned(),
            Self::Monthly => "monthly".to_owned(),
            Self::Custom { seconds } => format!("custom_{seconds}s"),
        }
    }
}

impl std::fmt::Display for QuotaWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// Behavior when a tenant exceeds their quota limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OverageBehavior {
    /// Block the action entirely (HTTP 429).
    Block,
    /// Allow the action but emit a warning header and metric.
    Warn,
    /// Degrade to a lower-priority fallback provider.
    Degrade {
        /// Provider to route to when quota is exceeded.
        fallback_provider: String,
    },
    /// Allow the action and send a notification to the tenant admin.
    Notify {
        /// Notification target (e.g., email address or webhook URL).
        target: String,
    },
}

impl std::fmt::Display for OverageBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Block => f.write_str("block"),
            Self::Warn => f.write_str("warn"),
            Self::Degrade { .. } => f.write_str("degrade"),
            Self::Notify { .. } => f.write_str("notify"),
        }
    }
}

/// A quota policy defining the usage limit for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QuotaPolicy {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Namespace this quota applies to.
    pub namespace: String,
    /// Tenant this quota applies to.
    pub tenant: String,
    /// Maximum number of actions allowed per window.
    pub max_actions: u64,
    /// Time window for the quota.
    pub window: QuotaWindow,
    /// What happens when the quota is exceeded.
    pub overage_behavior: OverageBehavior,
    /// Whether this quota policy is currently active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// When this quota policy was created.
    pub created_at: DateTime<Utc>,
    /// When this quota policy was last updated.
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

/// Current quota usage for a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QuotaUsage {
    /// Tenant identifier.
    pub tenant: String,
    /// Namespace.
    pub namespace: String,
    /// Number of actions used in the current window.
    pub used: u64,
    /// Maximum actions allowed.
    pub limit: u64,
    /// Remaining actions before the quota is reached.
    pub remaining: u64,
    /// The quota window type.
    pub window: QuotaWindow,
    /// When the current window resets.
    pub resets_at: DateTime<Utc>,
    /// Overage behavior configured for this quota.
    pub overage_behavior: OverageBehavior,
}

/// Compute the start of the current quota window and when it resets.
///
/// Returns `(window_start, window_end)` based on the window type and current time.
///
/// # Panics
///
/// Panics if the window duration is zero (which should be prevented by
/// validation at the API layer).
#[must_use]
pub fn compute_window_boundaries(
    window: &QuotaWindow,
    now: &DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let secs = window.duration_seconds();
    assert!(secs > 0, "quota window duration must be greater than 0");
    let duration = chrono::Duration::seconds(secs.cast_signed());
    // Use epoch-aligned windows so all instances agree on boundaries.
    let epoch = DateTime::UNIX_EPOCH;
    let elapsed = now.signed_duration_since(epoch);
    let window_secs = secs.cast_signed();
    let window_index = elapsed.num_seconds() / window_secs;
    let window_start = epoch + chrono::Duration::seconds(window_index * window_secs);
    let window_end = window_start + duration;
    (window_start, window_end)
}

/// Build a state key suffix for a quota usage counter.
///
/// Format: `{namespace}:{tenant}:{window_label}:{window_index}`
///
/// # Panics
///
/// Panics if the window duration is zero.
#[must_use]
pub fn quota_counter_key(
    namespace: &str,
    tenant: &str,
    window: &QuotaWindow,
    now: &DateTime<Utc>,
) -> String {
    let secs = window.duration_seconds();
    assert!(secs > 0, "quota window duration must be greater than 0");
    let epoch = DateTime::UNIX_EPOCH;
    let elapsed = now.signed_duration_since(epoch);
    let window_secs = secs.cast_signed();
    let window_index = elapsed.num_seconds() / window_secs;
    format!("{namespace}:{tenant}:{}:{window_index}", window.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_window_duration() {
        assert_eq!(QuotaWindow::Hourly.duration_seconds(), 3_600);
        assert_eq!(QuotaWindow::Daily.duration_seconds(), 86_400);
        assert_eq!(QuotaWindow::Weekly.duration_seconds(), 604_800);
        assert_eq!(QuotaWindow::Monthly.duration_seconds(), 2_592_000);
        assert_eq!(
            QuotaWindow::Custom { seconds: 7200 }.duration_seconds(),
            7200
        );
    }

    #[test]
    fn quota_window_label() {
        assert_eq!(QuotaWindow::Hourly.label(), "hourly");
        assert_eq!(QuotaWindow::Daily.label(), "daily");
        assert_eq!(QuotaWindow::Weekly.label(), "weekly");
        assert_eq!(QuotaWindow::Monthly.label(), "monthly");
        assert_eq!(
            QuotaWindow::Custom { seconds: 7200 }.label(),
            "custom_7200s"
        );
    }

    #[test]
    fn quota_window_display() {
        assert_eq!(format!("{}", QuotaWindow::Hourly), "hourly");
        assert_eq!(
            format!("{}", QuotaWindow::Custom { seconds: 300 }),
            "custom_300s"
        );
    }

    #[test]
    fn overage_behavior_display() {
        assert_eq!(format!("{}", OverageBehavior::Block), "block");
        assert_eq!(format!("{}", OverageBehavior::Warn), "warn");
        assert_eq!(
            format!(
                "{}",
                OverageBehavior::Degrade {
                    fallback_provider: "log".into()
                }
            ),
            "degrade"
        );
        assert_eq!(
            format!(
                "{}",
                OverageBehavior::Notify {
                    target: "admin@test.com".into()
                }
            ),
            "notify"
        );
    }

    #[test]
    fn quota_policy_serde_roundtrip() {
        let policy = QuotaPolicy {
            id: "q-001".into(),
            namespace: "notifications".into(),
            tenant: "tenant-1".into(),
            max_actions: 1000,
            window: QuotaWindow::Daily,
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: Some("Daily limit for tenant-1".into()),
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let back: QuotaPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "q-001");
        assert_eq!(back.max_actions, 1000);
        assert_eq!(back.window, QuotaWindow::Daily);
        assert_eq!(back.overage_behavior, OverageBehavior::Block);
        assert!(back.enabled);
    }

    #[test]
    fn quota_policy_deserializes_with_defaults() {
        let json = r#"{
            "id": "q-002",
            "namespace": "ns",
            "tenant": "t",
            "max_actions": 500,
            "window": "hourly",
            "overage_behavior": "warn",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;

        let policy: QuotaPolicy = serde_json::from_str(json).unwrap();
        assert!(policy.enabled);
        assert!(policy.description.is_none());
        assert!(policy.labels.is_empty());
    }

    #[test]
    fn quota_usage_serde_roundtrip() {
        let usage = QuotaUsage {
            tenant: "t".into(),
            namespace: "ns".into(),
            used: 42,
            limit: 100,
            remaining: 58,
            window: QuotaWindow::Hourly,
            resets_at: Utc::now() + chrono::Duration::minutes(30),
            overage_behavior: OverageBehavior::Warn,
        };

        let json = serde_json::to_string(&usage).unwrap();
        let back: QuotaUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.used, 42);
        assert_eq!(back.limit, 100);
        assert_eq!(back.remaining, 58);
    }

    #[test]
    fn overage_behavior_degrade_serde() {
        let behavior = OverageBehavior::Degrade {
            fallback_provider: "log".into(),
        };
        let json = serde_json::to_string(&behavior).unwrap();
        let back: OverageBehavior = serde_json::from_str(&json).unwrap();
        assert_eq!(back, behavior);
    }

    #[test]
    fn overage_behavior_notify_serde() {
        let behavior = OverageBehavior::Notify {
            target: "admin@example.com".into(),
        };
        let json = serde_json::to_string(&behavior).unwrap();
        let back: OverageBehavior = serde_json::from_str(&json).unwrap();
        assert_eq!(back, behavior);
    }

    #[test]
    fn compute_window_boundaries_aligned() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-02-10T14:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (start, end) = compute_window_boundaries(&QuotaWindow::Hourly, &now);
        assert_eq!(start.format("%H:%M:%S").to_string(), "14:00:00");
        assert_eq!(end.format("%H:%M:%S").to_string(), "15:00:00");
    }

    #[test]
    fn compute_window_boundaries_daily() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-02-10T14:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (start, end) = compute_window_boundaries(&QuotaWindow::Daily, &now);
        assert_eq!(
            start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2026-02-10T00:00:00"
        );
        assert_eq!(
            end.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2026-02-11T00:00:00"
        );
    }

    #[test]
    fn quota_counter_key_format() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-02-10T14:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let key = quota_counter_key("ns", "tenant-1", &QuotaWindow::Hourly, &now);
        assert!(key.starts_with("ns:tenant-1:hourly:"));
        // Same time should produce the same key.
        let key2 = quota_counter_key("ns", "tenant-1", &QuotaWindow::Hourly, &now);
        assert_eq!(key, key2);
    }

    #[test]
    fn quota_counter_key_different_windows() {
        let now = Utc::now();
        let k1 = quota_counter_key("ns", "t", &QuotaWindow::Hourly, &now);
        let k2 = quota_counter_key("ns", "t", &QuotaWindow::Daily, &now);
        assert_ne!(k1, k2);
    }

    #[test]
    fn quota_window_custom_serde() {
        let window = QuotaWindow::Custom { seconds: 7200 };
        let json = serde_json::to_string(&window).unwrap();
        let back: QuotaWindow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, window);
    }

    #[test]
    fn quota_policy_with_all_fields() {
        let mut labels = HashMap::new();
        labels.insert("tier".into(), "premium".into());

        let policy = QuotaPolicy {
            id: "q-full".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            max_actions: 10_000,
            window: QuotaWindow::Monthly,
            overage_behavior: OverageBehavior::Degrade {
                fallback_provider: "log".into(),
            },
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: Some("Premium tier monthly quota".into()),
            labels,
        };

        let json = serde_json::to_string_pretty(&policy).unwrap();
        let back: QuotaPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_actions, 10_000);
        assert_eq!(back.labels.get("tier"), Some(&"premium".to_string()));
        assert!(matches!(
            back.overage_behavior,
            OverageBehavior::Degrade { .. }
        ));
    }

    #[test]
    fn quota_policy_disabled() {
        let policy = QuotaPolicy {
            id: "q-dis".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            max_actions: 100,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Block,
            enabled: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };

        let json = serde_json::to_string(&policy).unwrap();
        let back: QuotaPolicy = serde_json::from_str(&json).unwrap();
        assert!(!back.enabled);
    }

    #[test]
    #[should_panic(expected = "quota window duration must be greater than 0")]
    fn compute_window_boundaries_panics_on_zero() {
        let now = Utc::now();
        let _ = compute_window_boundaries(&QuotaWindow::Custom { seconds: 0 }, &now);
    }

    #[test]
    #[should_panic(expected = "quota window duration must be greater than 0")]
    fn quota_counter_key_panics_on_zero() {
        let now = Utc::now();
        let _ = quota_counter_key("ns", "t", &QuotaWindow::Custom { seconds: 0 }, &now);
    }
}
