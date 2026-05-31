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
///
/// A policy can be **generic** (applies to every dispatch for the
/// `(namespace, tenant)` pair regardless of target provider or
/// caller) or **scoped** along the `provider` and/or `principal`
/// dimensions. Multiple policies may coexist for the same
/// `(namespace, tenant)`; all of them are evaluated in parallel and
/// the **strictest** result wins. Typical uses: stack a daily
/// tenant-wide cap with per-provider burst caps, or carve out a
/// per-API-key budget on top of the tenant total.
///
/// | Policy | Matches | Counted against |
/// |---|---|---|
/// | `provider: None`, `principal: None` | Any dispatch for the tenant | Tenant-wide bucket |
/// | `provider: Some("slack")` | Only dispatches to the `slack` provider | Provider bucket |
/// | `principal: Some("svc-billing")` | Only dispatches by caller `svc-billing` | Principal bucket |
/// | both set | Only dispatches by that caller to that provider | Per-(principal,provider) bucket |
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QuotaPolicy {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Namespace this quota applies to.
    pub namespace: String,
    /// Tenant this quota applies to.
    pub tenant: String,
    /// Optional provider this quota applies to. When `None`, the
    /// policy is generic and applies to every dispatch for the
    /// `(namespace, tenant)` pair. When `Some(provider)`, only
    /// dispatches whose `action.provider` equals this value count
    /// against (and are enforced by) this policy.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional caller identifier (API key name, JWT subject, etc.)
    /// this quota applies to. When `None`, the policy ignores the
    /// caller and counts all dispatches for its scope. When
    /// `Some(principal)`, only dispatches made by that caller count
    /// against (and are enforced by) this policy.
    ///
    /// Dispatches without an authenticated caller (background jobs,
    /// chain steps, scheduled re-dispatches) never match a
    /// principal-scoped policy and so are not affected by it.
    #[serde(default)]
    pub principal: Option<String>,
    /// Whether this policy applies per-principal. When true, a
    /// separate counter is maintained for every unique principal
    /// that matches the policy's other dimensions.
    ///
    /// This enables "Each user gets X/day" policies without creating
    /// a separate record for every user. If `principal` is also set,
    /// this flag is ignored and the policy is pinned to that
    /// specific principal.
    #[serde(default)]
    pub per_principal: bool,
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

/// Maximum length allowed for `namespace`, `tenant`, and `provider`
/// identifiers used in quota scopes. Prevents unbounded state-key
/// growth and bounds validation work.
pub const MAX_QUOTA_IDENTIFIER_LEN: usize = 128;

/// Maximum number of quota policies permitted per `(namespace,
/// tenant)` bucket. One generic plus up to 31 per-provider caps is
/// ample for real deployments and bounds cold-path load work plus
/// per-dispatch iteration cost (mitigating policy-explosion `DoS`).
pub const MAX_POLICIES_PER_BUCKET: usize = 32;

/// Errors reported by [`validate_quota_scope_identifier`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaIdentifierError {
    /// The identifier was empty.
    Empty,
    /// The identifier exceeded [`MAX_QUOTA_IDENTIFIER_LEN`].
    TooLong(usize),
    /// The identifier contained a reserved separator character (`:`)
    /// that would enable state-key injection / cross-tenant counter
    /// collisions.
    ReservedChar(char),
    /// The identifier contained an ASCII control character.
    ControlChar(char),
}

impl std::fmt::Display for QuotaIdentifierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("identifier must not be empty"),
            Self::TooLong(n) => write!(
                f,
                "identifier length {n} exceeds maximum {MAX_QUOTA_IDENTIFIER_LEN}"
            ),
            Self::ReservedChar(c) => {
                write!(f, "identifier must not contain reserved character {c:?}")
            }
            Self::ControlChar(c) => write!(
                f,
                "identifier must not contain control character U+{:04X}",
                *c as u32
            ),
        }
    }
}

impl std::error::Error for QuotaIdentifierError {}

/// Validate a namespace/tenant/provider identifier used in a quota
/// scope, rejecting values that could collide with the counter-key
/// separator or inflate state-store keys without bound.
///
/// # Errors
///
/// Returns [`QuotaIdentifierError`] if the identifier is empty,
/// exceeds [`MAX_QUOTA_IDENTIFIER_LEN`], contains the reserved
/// separator `:`, or contains any ASCII control character.
pub fn validate_quota_scope_identifier(s: &str) -> Result<(), QuotaIdentifierError> {
    if s.is_empty() {
        return Err(QuotaIdentifierError::Empty);
    }
    if s.len() > MAX_QUOTA_IDENTIFIER_LEN {
        return Err(QuotaIdentifierError::TooLong(s.len()));
    }
    for c in s.chars() {
        if c == ':' {
            return Err(QuotaIdentifierError::ReservedChar(c));
        }
        if c.is_control() {
            return Err(QuotaIdentifierError::ControlChar(c));
        }
    }
    Ok(())
}

impl QuotaPolicy {
    /// Whether this policy applies to an action dispatched to the
    /// given provider. A generic policy (`provider: None`) applies
    /// to every dispatch; a provider-scoped policy applies only
    /// when the provider matches exactly.
    #[must_use]
    pub fn applies_to_provider(&self, provider: &str) -> bool {
        match &self.provider {
            None => true,
            Some(p) => p.as_str() == provider,
        }
    }

    /// Whether this policy applies to a dispatch made by the given
    /// caller. A policy without a `principal` scope (`principal:
    /// None`) applies to every caller. A principal-scoped policy
    /// applies only when the caller's id matches exactly; dispatches
    /// without an authenticated caller (`principal = None` argument)
    /// never match a principal-scoped policy.
    ///
    /// If `per_principal` is true, the policy matches any
    /// authenticated caller and maintains a separate counter for
    /// each.
    #[must_use]
    pub fn applies_to_principal(&self, principal: Option<&str>) -> bool {
        if self.principal.is_none() && self.per_principal {
            return principal.is_some();
        }
        match (&self.principal, principal) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(scoped), Some(actual)) => scoped.as_str() == actual,
        }
    }

    /// Validate that this policy's scope identifiers are safe to
    /// use as state-store key components and that the time window
    /// and `max_actions` are non-zero. Callers use this both at
    /// creation time and when loading records from the state
    /// store, so a corrupt or legacy record is rejected before it
    /// can produce unsafe keys or trigger a zero-window
    /// arithmetic panic downstream.
    ///
    /// # Errors
    ///
    /// Returns an error string describing the first validation
    /// failure encountered.
    pub fn validate_scope(&self) -> Result<(), String> {
        validate_quota_scope_identifier(&self.namespace)
            .map_err(|e| format!("invalid namespace: {e}"))?;
        validate_quota_scope_identifier(&self.tenant)
            .map_err(|e| format!("invalid tenant: {e}"))?;
        if let Some(ref p) = self.provider {
            validate_quota_scope_identifier(p).map_err(|e| format!("invalid provider: {e}"))?;
        }
        if let Some(ref p) = self.principal {
            validate_quota_scope_identifier(p).map_err(|e| format!("invalid principal: {e}"))?;
        }
        if self.window.duration_seconds() == 0 {
            return Err("quota window duration must be greater than 0".to_string());
        }
        if self.max_actions == 0 {
            return Err("max_actions must be greater than 0".to_string());
        }
        Ok(())
    }
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

/// Upper bound on a quota window length, in seconds (~100 years).
///
/// Bounds the window so the boundary arithmetic in
/// [`compute_window_boundaries`] cannot overflow the [`DateTime`] range and
/// panic. The API layer rejects larger custom windows up front; the clamp in
/// `compute_window_boundaries` is a defensive backstop for any value that
/// reaches the core directly (e.g. a policy persisted before this bound).
pub const MAX_WINDOW_SECONDS: u64 = 100 * 366 * 86_400;

/// Compute the start of the current quota window and when it resets.
///
/// Returns `(window_start, window_end)` based on the window type and current
/// time. The window length is clamped to `[1, MAX_WINDOW_SECONDS]` so this
/// never panics — a zero window (division by zero) or an astronomically large
/// one (overflowing the `DateTime` range) is clamped rather than aborting the
/// caller. The quota-usage endpoint must not be a panic/DoS surface.
#[must_use]
pub fn compute_window_boundaries(
    window: &QuotaWindow,
    now: &DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let window_secs = window
        .duration_seconds()
        .clamp(1, MAX_WINDOW_SECONDS)
        .cast_signed();
    let duration = chrono::Duration::seconds(window_secs);
    // Use epoch-aligned windows so all instances agree on boundaries.
    let epoch = DateTime::UNIX_EPOCH;
    let elapsed = now.signed_duration_since(epoch);
    let window_index = elapsed.num_seconds() / window_secs;
    let start_offset = window_index.saturating_mul(window_secs);
    let window_start = epoch
        .checked_add_signed(chrono::Duration::seconds(start_offset))
        .unwrap_or(epoch);
    let window_end = window_start
        .checked_add_signed(duration)
        .unwrap_or(window_start);
    (window_start, window_end)
}

/// Build a state key suffix for a quota usage counter.
///
/// Format: `{namespace}:{tenant}:{principal_or_*}:{provider_or_*}:{window_label}:{window_index}`
///
/// The `principal` and `provider` components are `"*"` when the
/// policy is unscoped along that dimension, and the literal
/// identifier otherwise. This guarantees that policies along
/// orthogonal dimensions (tenant-wide / per-provider /
/// per-principal / per-(principal,provider)) all maintain
/// independent counters — a burst of `slack` dispatches by one
/// API key does not consume the `email` budget or another caller's
/// budget, and a per-caller policy does not interfere with the
/// tenant-wide counter.
///
/// Returns `None` instead of panicking when any of these would
/// produce an unsafe or nonsensical key:
///
/// * The window duration is zero (would divide by zero).
/// * `namespace`, `tenant`, `provider`, or `principal` fails
///   [`validate_quota_scope_identifier`] (e.g., contains the
///   reserved `:` separator that would enable cross-tenant key
///   collisions).
///
/// Callers should treat `None` as fail-closed for the affected
/// policy — skip enforcement and log a warning so the offending
/// record can be repaired — rather than crashing the gateway.
#[must_use]
pub fn quota_counter_key(
    namespace: &str,
    tenant: &str,
    principal: Option<&str>,
    provider: Option<&str>,
    window: &QuotaWindow,
    now: &DateTime<Utc>,
) -> Option<String> {
    let secs = window.duration_seconds();
    if secs == 0 {
        return None;
    }
    if validate_quota_scope_identifier(namespace).is_err()
        || validate_quota_scope_identifier(tenant).is_err()
    {
        return None;
    }
    if let Some(p) = provider
        && validate_quota_scope_identifier(p).is_err()
    {
        return None;
    }
    if let Some(p) = principal
        && validate_quota_scope_identifier(p).is_err()
    {
        return None;
    }
    let epoch = DateTime::UNIX_EPOCH;
    let elapsed = now.signed_duration_since(epoch);
    let window_secs = secs.cast_signed();
    let window_index = elapsed.num_seconds() / window_secs;
    let principal_part = principal.unwrap_or("*");
    let provider_part = provider.unwrap_or("*");
    Some(format!(
        "{namespace}:{tenant}:{principal_part}:{provider_part}:{}:{window_index}",
        window.label()
    ))
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
    fn compute_window_boundaries_never_panics_on_extreme_windows() {
        let now = Utc::now();

        // Astronomically large custom window: must clamp, not overflow/panic.
        let (start, end) =
            compute_window_boundaries(&QuotaWindow::Custom { seconds: u64::MAX }, &now);
        assert!(end >= start, "window end must not precede start");
        assert!(
            start <= now,
            "epoch-aligned start must not be in the future"
        );

        // Just over the cap clamps to the cap; just under is honoured exactly.
        let (_, end_capped) = compute_window_boundaries(
            &QuotaWindow::Custom {
                seconds: MAX_WINDOW_SECONDS + 1,
            },
            &now,
        );
        let (_, end_at_cap) = compute_window_boundaries(
            &QuotaWindow::Custom {
                seconds: MAX_WINDOW_SECONDS,
            },
            &now,
        );
        assert_eq!(end_capped, end_at_cap);

        // Zero window (e.g. a stale stored policy) clamps to 1s instead of a
        // divide-by-zero panic.
        let (z_start, z_end) = compute_window_boundaries(&QuotaWindow::Custom { seconds: 0 }, &now);
        assert!(z_end > z_start);

        // A normal window is unaffected by the clamp.
        let (h_start, h_end) = compute_window_boundaries(&QuotaWindow::Hourly, &now);
        assert_eq!((h_end - h_start).num_seconds(), 3_600);
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
            provider: None,
            principal: None,
            per_principal: false,
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
        let key =
            quota_counter_key("ns", "tenant-1", None, None, &QuotaWindow::Hourly, &now).unwrap();
        // Generic (no principal, no provider) policies encode as "*:*".
        assert!(key.starts_with("ns:tenant-1:*:*:hourly:"));
        // Same time should produce the same key.
        let key2 =
            quota_counter_key("ns", "tenant-1", None, None, &QuotaWindow::Hourly, &now).unwrap();
        assert_eq!(key, key2);
    }

    #[test]
    fn quota_counter_key_per_provider_isolation() {
        let now = Utc::now();
        let generic = quota_counter_key("ns", "t", None, None, &QuotaWindow::Hourly, &now).unwrap();
        let slack =
            quota_counter_key("ns", "t", None, Some("slack"), &QuotaWindow::Hourly, &now).unwrap();
        let email =
            quota_counter_key("ns", "t", None, Some("email"), &QuotaWindow::Hourly, &now).unwrap();
        // All three live in separate counter buckets.
        assert_ne!(generic, slack);
        assert_ne!(generic, email);
        assert_ne!(slack, email);
        assert!(slack.contains(":slack:"));
        assert!(email.contains(":email:"));
    }

    #[test]
    fn quota_counter_key_per_principal_isolation() {
        let now = Utc::now();
        let generic = quota_counter_key("ns", "t", None, None, &QuotaWindow::Hourly, &now).unwrap();
        let alice =
            quota_counter_key("ns", "t", Some("alice"), None, &QuotaWindow::Hourly, &now).unwrap();
        let bob =
            quota_counter_key("ns", "t", Some("bob"), None, &QuotaWindow::Hourly, &now).unwrap();
        let alice_slack = quota_counter_key(
            "ns",
            "t",
            Some("alice"),
            Some("slack"),
            &QuotaWindow::Hourly,
            &now,
        )
        .unwrap();
        // Each (principal, provider) combination keys its own bucket.
        assert_ne!(generic, alice);
        assert_ne!(alice, bob);
        assert_ne!(alice, alice_slack);
        assert!(alice.contains(":alice:*:"));
        assert!(alice_slack.contains(":alice:slack:"));
    }

    #[test]
    fn quota_counter_key_different_windows() {
        let now = Utc::now();
        let k1 = quota_counter_key("ns", "t", None, None, &QuotaWindow::Hourly, &now);
        let k2 = quota_counter_key("ns", "t", None, None, &QuotaWindow::Daily, &now);
        assert_ne!(k1, k2);
    }

    #[test]
    fn quota_counter_key_returns_none_on_zero_window() {
        let now = Utc::now();
        assert!(
            quota_counter_key(
                "ns",
                "t",
                None,
                None,
                &QuotaWindow::Custom { seconds: 0 },
                &now
            )
            .is_none(),
            "zero window must return None (fail-closed) instead of panicking"
        );
    }

    #[test]
    fn quota_counter_key_rejects_colon_in_identifiers() {
        // Cross-tenant key injection attempt: a malicious provider or
        // principal name containing `:` must not produce a key that
        // could collide with another tenant's counter bucket.
        let now = Utc::now();
        assert!(
            quota_counter_key(
                "acme",
                "t",
                None,
                Some("slack:acme:*"),
                &QuotaWindow::Hourly,
                &now
            )
            .is_none()
        );
        assert!(
            quota_counter_key(
                "acme",
                "t",
                Some("alice:rogue"),
                None,
                &QuotaWindow::Hourly,
                &now
            )
            .is_none()
        );
        assert!(
            quota_counter_key("ns:rogue", "t", None, None, &QuotaWindow::Hourly, &now).is_none()
        );
        assert!(
            quota_counter_key("ns", "t:rogue", None, None, &QuotaWindow::Hourly, &now).is_none()
        );
    }

    #[test]
    fn validate_quota_scope_identifier_accepts_typical_names() {
        for s in &[
            "acme",
            "acme-prod",
            "acme.us-east",
            "tenant_123",
            "notifications",
            "auth0|user123",
            "user@example.com",
        ] {
            assert!(
                validate_quota_scope_identifier(s).is_ok(),
                "should accept {s:?}"
            );
        }
    }

    #[test]
    fn validate_quota_scope_identifier_rejects_dangerous_input() {
        assert_eq!(
            validate_quota_scope_identifier(""),
            Err(QuotaIdentifierError::Empty)
        );
        let long = "a".repeat(MAX_QUOTA_IDENTIFIER_LEN + 1);
        assert!(matches!(
            validate_quota_scope_identifier(&long),
            Err(QuotaIdentifierError::TooLong(_))
        ));
        assert!(matches!(
            validate_quota_scope_identifier("foo:bar"),
            Err(QuotaIdentifierError::ReservedChar(':'))
        ));
        assert!(matches!(
            validate_quota_scope_identifier("foo\nbar"),
            Err(QuotaIdentifierError::ControlChar(_))
        ));
        assert!(matches!(
            validate_quota_scope_identifier("foo\0bar"),
            Err(QuotaIdentifierError::ControlChar(_))
        ));
    }

    #[test]
    fn quota_policy_validate_scope_catches_corrupt_records() {
        let mut policy = QuotaPolicy {
            id: "q-1".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            provider: None,
            principal: None,
            per_principal: false,
            max_actions: 100,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        assert!(policy.validate_scope().is_ok());

        // Zero window is rejected (would panic elsewhere).
        policy.window = QuotaWindow::Custom { seconds: 0 };
        assert!(policy.validate_scope().is_err());

        // Colon in provider is rejected (key injection).
        policy.window = QuotaWindow::Hourly;
        policy.provider = Some("bad:provider".into());
        assert!(policy.validate_scope().is_err());

        // Colon in principal is rejected (key injection).
        policy.provider = None;
        policy.principal = Some("bad:principal".into());
        assert!(policy.validate_scope().is_err());

        // Zero max_actions is rejected.
        policy.principal = None;
        policy.max_actions = 0;
        assert!(policy.validate_scope().is_err());
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
            provider: Some("slack".into()),
            principal: None,
            per_principal: false,
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
        assert_eq!(back.provider.as_deref(), Some("slack"));
        assert!(matches!(
            back.overage_behavior,
            OverageBehavior::Degrade { .. }
        ));
    }

    #[test]
    fn quota_policy_applies_to_provider() {
        let mut generic = QuotaPolicy {
            id: "q-generic".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            provider: None,
            principal: None,
            per_principal: false,
            max_actions: 100,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        // Generic policy applies to every provider.
        assert!(generic.applies_to_provider("slack"));
        assert!(generic.applies_to_provider("email"));
        assert!(generic.applies_to_provider("webhook"));

        // Scoping to a single provider only matches that provider.
        generic.provider = Some("slack".into());
        assert!(generic.applies_to_provider("slack"));
        assert!(!generic.applies_to_provider("email"));
        assert!(!generic.applies_to_provider("webhook"));
    }

    #[test]
    fn quota_policy_defaults_provider_to_none() {
        // Records written before Phase 3 don't carry the provider field;
        // they should deserialize as generic (None) policies for
        // backward compat.
        let json = r#"{
            "id": "q-legacy",
            "namespace": "ns",
            "tenant": "t",
            "max_actions": 500,
            "window": "daily",
            "overage_behavior": "block",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let policy: QuotaPolicy = serde_json::from_str(json).unwrap();
        assert!(policy.provider.is_none());
        assert!(policy.applies_to_provider("anything"));
    }

    #[test]
    fn quota_policy_applies_to_principal() {
        let mut policy = QuotaPolicy {
            id: "q-p".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            provider: None,
            principal: None,
            per_principal: false,
            max_actions: 100,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: None,
            labels: HashMap::new(),
        };
        // Unscoped policy matches every caller, including anonymous.
        assert!(policy.applies_to_principal(Some("alice")));
        assert!(policy.applies_to_principal(Some("svc-billing")));
        assert!(policy.applies_to_principal(None));

        // Principal-scoped policy only matches the exact caller, and
        // never matches an unauthenticated dispatch.
        policy.principal = Some("alice".into());
        assert!(policy.applies_to_principal(Some("alice")));
        assert!(!policy.applies_to_principal(Some("bob")));
        assert!(!policy.applies_to_principal(None));

        // Dynamic per-principal policy matches any authenticated caller,
        // but never matches an unauthenticated dispatch.
        policy.principal = None;
        policy.per_principal = true;
        assert!(policy.applies_to_principal(Some("alice")));
        assert!(policy.applies_to_principal(Some("bob")));
        assert!(!policy.applies_to_principal(None));
    }

    #[test]
    fn quota_policy_defaults_principal_to_none() {
        let json = r#"{
            "id": "q-legacy-p",
            "namespace": "ns",
            "tenant": "t",
            "max_actions": 500,
            "window": "daily",
            "overage_behavior": "block",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }"#;
        let policy: QuotaPolicy = serde_json::from_str(json).unwrap();
        assert!(policy.principal.is_none());
        assert!(policy.applies_to_principal(Some("anyone")));
        assert!(policy.applies_to_principal(None));
    }

    #[test]
    fn quota_policy_disabled() {
        let policy = QuotaPolicy {
            id: "q-dis".into(),
            namespace: "ns".into(),
            tenant: "t".into(),
            provider: None,
            principal: None,
            per_principal: false,
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
    fn compute_window_boundaries_clamps_zero_window() {
        // A zero window used to panic (divide-by-zero); it now clamps to a 1s
        // window so the usage endpoint stays available. Covered in detail by
        // `compute_window_boundaries_never_panics_on_extreme_windows`.
        let now = Utc::now();
        let (start, end) = compute_window_boundaries(&QuotaWindow::Custom { seconds: 0 }, &now);
        assert!(end > start);
    }
}
