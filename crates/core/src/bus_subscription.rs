//! Bus subscription — durable Kafka consumer-group binding.
//!
//! A `Subscription` is Acteon's record of a long-lived consumer group
//! plus the operator-facing metadata around it (filter, DLQ, ack
//! semantics). Kafka owns offsets and partition assignment; Acteon
//! owns identity, DLQ routing, and policy.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a fresh subscription should begin reading if the group has no
/// committed offset yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartOffset {
    /// Start at the earliest retained record.
    Earliest,
    /// Start at the broker's high-water mark (default).
    #[default]
    Latest,
}

/// How the subscription acknowledges messages it has processed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckMode {
    /// Offset is committed automatically after each message is delivered
    /// to a consumer. Lower safety, highest throughput.
    AutoOnDelivery,
    /// Consumer must explicitly call `ack` for each offset it accepts.
    /// Default; survives consumer crashes.
    #[default]
    Manual,
}

/// Operator-visible state of the durable consumer behind a subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    /// Consumer task is idle — no one has attached to the stream yet.
    #[default]
    Inactive,
    /// Consumer task is running and delivering records.
    Active,
    /// Consumer task terminated with an error; the registry will try to
    /// revive it on the next attach.
    Errored,
}

/// Durable Kafka-backed subscription tracked by Acteon.
///
/// Not `ToSchema` — the API layer has its own DTOs. Keeping the
/// internal state type free of `OpenAPI` derives lets us evolve it
/// independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    /// Stable identifier; doubles as the Kafka `group.id`.
    pub id: String,
    /// Target Kafka topic (full `namespace.tenant.name` form).
    pub topic: String,
    /// Namespace / tenant the subscription belongs to (for ACL + state
    /// scoping). Must match the topic's namespace/tenant.
    pub namespace: String,
    pub tenant: String,
    /// Starting offset if the consumer group has no committed offset.
    #[serde(default)]
    pub starting_offset: StartOffset,
    /// Ack model.
    #[serde(default)]
    pub ack_mode: AckMode,
    /// Optional Kafka-name of the dead-letter topic to route failures to.
    /// When `None`, a failed delivery is retried by Kafka's rebalance.
    #[serde(default)]
    pub dead_letter_topic: Option<String>,
    /// Per-delivery ack timeout. Messages un-acked after this much time
    /// are routed to the DLQ (if configured) and marked failed.
    #[serde(default = "default_ack_timeout_ms")]
    pub ack_timeout_ms: u64,
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary operator labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Created / updated timestamps for the state row.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_ack_timeout_ms() -> u64 {
    // 30s matches the executor's per-provider default — long enough for
    // most LLM calls, short enough that stuck consumers don't hold up
    // an entire partition.
    30_000
}

impl Subscription {
    /// Construct a subscription with sensible defaults.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        topic: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            topic: topic.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            starting_offset: StartOffset::default(),
            ack_mode: AckMode::default(),
            dead_letter_topic: None,
            ack_timeout_ms: default_ack_timeout_ms(),
            description: None,
            labels: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Valid characters: same rules as topic fragments — the subscription
    /// ID is also used as the Kafka `group.id`, which accepts a superset
    /// but Acteon tightens to the topic-fragment rule for consistency.
    pub fn validate_id(s: &str) -> Result<(), SubscriptionValidationError> {
        if s.is_empty() {
            return Err(SubscriptionValidationError::EmptyId);
        }
        if s.len() > 120 {
            return Err(SubscriptionValidationError::IdTooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SubscriptionValidationError::InvalidIdChar(s.into()));
        }
        Ok(())
    }

    /// Validate the subscription end-to-end.
    pub fn validate(&self) -> Result<(), SubscriptionValidationError> {
        Self::validate_id(&self.id)?;
        if self.ack_timeout_ms == 0 {
            return Err(SubscriptionValidationError::ZeroAckTimeout);
        }
        if self.topic.is_empty() {
            return Err(SubscriptionValidationError::EmptyTopic);
        }
        Ok(())
    }
}

/// Lag snapshot returned by the `/lag` endpoint. One entry per partition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionLag {
    pub partition: i32,
    /// Committed offset for this group on this partition (`-1` if none).
    pub committed: i64,
    /// Current broker high-water mark for this partition.
    pub high_water_mark: i64,
    /// `high_water_mark - committed` (0 if uncommitted).
    pub lag: i64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubscriptionValidationError {
    #[error("subscription id must not be empty")]
    EmptyId,
    #[error("subscription id exceeds 120 characters")]
    IdTooLong,
    #[error("subscription id '{0}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidIdChar(String),
    #[error("ack_timeout_ms must be > 0")]
    ZeroAckTimeout,
    #[error("topic must not be empty")]
    EmptyTopic,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Subscription::new("sub-1", "ns.t.orders", "ns", "t");
        assert!(s.validate().is_ok());
        assert_eq!(s.starting_offset, StartOffset::Latest);
        assert_eq!(s.ack_mode, AckMode::Manual);
        assert_eq!(s.ack_timeout_ms, 30_000);
    }

    #[test]
    fn validate_rejects_bad_id() {
        let mut s = Subscription::new("has.dots", "t", "ns", "tn");
        assert!(matches!(
            s.validate(),
            Err(SubscriptionValidationError::InvalidIdChar(_))
        ));
        s.id = String::new();
        assert_eq!(s.validate(), Err(SubscriptionValidationError::EmptyId));
    }

    #[test]
    fn validate_rejects_zero_timeout() {
        let mut s = Subscription::new("sub-1", "t", "ns", "tn");
        s.ack_timeout_ms = 0;
        assert_eq!(
            s.validate(),
            Err(SubscriptionValidationError::ZeroAckTimeout)
        );
    }

    #[test]
    fn roundtrip_serde() {
        let mut s = Subscription::new("sub-1", "ns.t.orders", "ns", "t");
        s.dead_letter_topic = Some("ns.t.orders-dlq".into());
        let json = serde_json::to_string(&s).unwrap();
        let back: Subscription = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dead_letter_topic.as_deref(), Some("ns.t.orders-dlq"));
    }
}
