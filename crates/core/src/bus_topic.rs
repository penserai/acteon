//! Bus topic — metadata for a Kafka-backed topic in the agentic bus.
//!
//! A `Topic` is the Acteon-side state object that pairs with a real Kafka
//! topic. Kafka owns the partitions, retention, and transport; Acteon owns
//! the name → ACL / schema / policy binding.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Acteon's view of a Kafka-backed bus topic.
///
/// The Kafka topic name is derived from `namespace`, `tenant`, and `name`
/// using [`Topic::kafka_topic_name`] so tenants are isolated at the
/// transport layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Topic {
    /// Short topic name inside the namespace/tenant (e.g. `"orders"`).
    pub name: String,
    /// Namespace the topic belongs to.
    pub namespace: String,
    /// Tenant that owns the topic.
    pub tenant: String,
    /// Kafka partition count. Immutable after creation; changes require a
    /// fresh topic.
    #[serde(default = "default_partitions")]
    pub partitions: i32,
    /// Replication factor used when creating the Kafka topic. Typically 1
    /// for single-broker dev, 3 for production.
    #[serde(default = "default_replication_factor")]
    pub replication_factor: i16,
    /// Retention in milliseconds. `None` means "inherit Kafka default."
    #[serde(default)]
    pub retention_ms: Option<i64>,
    /// Schema subject this topic is bound to (filled in by Phase 3).
    #[serde(default)]
    pub schema_subject: Option<String>,
    /// Schema version pin (filled in by Phase 3). `None` = latest.
    #[serde(default)]
    pub schema_version: Option<i32>,
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Arbitrary operator labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// When the topic object was created in Acteon state.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

fn default_partitions() -> i32 {
    3
}

fn default_replication_factor() -> i16 {
    1
}

impl Topic {
    /// Construct a topic with sensible defaults.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        namespace: impl Into<String>,
        tenant: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            partitions: default_partitions(),
            replication_factor: default_replication_factor(),
            retention_ms: None,
            schema_subject: None,
            schema_version: None,
            description: None,
            labels: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// The **canonical Kafka topic name** derived from
    /// `namespace.tenant.name`. Kafka-forbidden characters are rejected at
    /// the publish edge via [`Self::validate_name`]; no escaping is done
    /// here by design so topic names stay human-readable.
    #[must_use]
    pub fn kafka_topic_name(&self) -> String {
        format!("{}.{}.{}", self.namespace, self.tenant, self.name)
    }

    /// Stable ID used as the state-store key. Matches the Kafka name so
    /// debugging is trivial.
    #[must_use]
    pub fn id(&self) -> String {
        self.kafka_topic_name()
    }

    /// Validate a topic name fragment (namespace / tenant / name).
    ///
    /// Kafka permits `[a-zA-Z0-9._-]`, max 249 chars total. We apply the
    /// same rule per-fragment and reserve `.` as a separator.
    pub fn validate_fragment(s: &str) -> Result<(), TopicValidationError> {
        if s.is_empty() {
            return Err(TopicValidationError::Empty);
        }
        if s.len() > 80 {
            return Err(TopicValidationError::TooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(TopicValidationError::InvalidChar(s.to_string()));
        }
        Ok(())
    }

    /// Validate this topic's fields end-to-end.
    pub fn validate(&self) -> Result<(), TopicValidationError> {
        Self::validate_fragment(&self.namespace)?;
        Self::validate_fragment(&self.tenant)?;
        Self::validate_fragment(&self.name)?;
        if self.partitions < 1 {
            return Err(TopicValidationError::InvalidPartitions(self.partitions));
        }
        if self.replication_factor < 1 {
            return Err(TopicValidationError::InvalidReplication(
                self.replication_factor,
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TopicValidationError {
    #[error("topic name fragment must not be empty")]
    Empty,
    #[error("topic name fragment exceeds 80 characters")]
    TooLong,
    #[error("topic name fragment '{0}' contains characters outside [a-zA-Z0-9_-]")]
    InvalidChar(String),
    #[error("partitions must be >= 1 (got {0})")]
    InvalidPartitions(i32),
    #[error("replication_factor must be >= 1 (got {0})")]
    InvalidReplication(i16),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kafka_topic_name_is_namespace_tenant_name() {
        let t = Topic::new("orders", "acme", "prod");
        assert_eq!(t.kafka_topic_name(), "acme.prod.orders");
    }

    #[test]
    fn validate_accepts_alphanumeric_and_dashes() {
        let t = Topic::new("tool-calls", "agents_01", "tenant-A");
        t.validate().unwrap();
    }

    #[test]
    fn validate_rejects_dots_in_fragment() {
        // Dots are the separator — not allowed in a fragment.
        let t = Topic::new("bad.name", "ns", "t");
        assert!(matches!(
            t.validate(),
            Err(TopicValidationError::InvalidChar(_))
        ));
    }

    #[test]
    fn validate_rejects_empty() {
        let t = Topic::new("", "ns", "t");
        assert_eq!(t.validate(), Err(TopicValidationError::Empty));
    }

    #[test]
    fn validate_rejects_zero_partitions() {
        let mut t = Topic::new("orders", "ns", "t");
        t.partitions = 0;
        assert!(matches!(
            t.validate(),
            Err(TopicValidationError::InvalidPartitions(0))
        ));
    }

    #[test]
    fn roundtrip_serde() {
        let t = Topic::new("orders", "acme", "prod");
        let json = serde_json::to_string(&t).unwrap();
        let back: Topic = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }
}
