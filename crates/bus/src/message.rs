use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a subscription should begin reading.
///
/// Phase 1 is deliberately minimal — Phase 2 promotes subscriptions
/// into a first-class type with persistent consumer-group offsets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartOffset {
    /// Begin at the earliest retained record.
    Earliest,
    /// Begin at the broker's high-water mark (default).
    #[default]
    Latest,
}

/// A single message flowing through the bus.
///
/// Produced messages carry only the envelope fields the caller cares
/// about; consumed messages additionally carry `partition`, `offset`,
/// and `timestamp` populated by the broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusMessage {
    /// Kafka topic the message belongs to (post-naming — the full
    /// `namespace.tenant.name` form from [`acteon_core::Topic`]).
    pub topic: String,
    /// Partition key. Messages with the same key land on the same
    /// partition (per-key ordering).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Message payload. Phase 1 carries opaque JSON; Phase 3 binds a
    /// schema.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// User-supplied headers. Reserved keys prefixed with `acteon.`
    /// are set by the publish edge and must not be used by callers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Partition assigned by the broker on produce. Populated only for
    /// consumed messages and for produce receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<i32>,
    /// Log offset within the partition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    /// Broker-assigned timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

impl BusMessage {
    /// Convenience constructor for producers.
    #[must_use]
    pub fn new(topic: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            topic: topic.into(),
            key: None,
            payload,
            headers: BTreeMap::new(),
            partition: None,
            offset: None,
            timestamp: None,
        }
    }

    /// Attach a partition key.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Attach a header. Reserved `acteon.*` keys are silently
    /// dropped to prevent callers from forging internal metadata.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        if !name.starts_with("acteon.") {
            self.headers.insert(name, value.into());
        }
        self
    }
}

/// Receipt returned from a produce call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub topic: String,
    pub partition: i32,
    pub offset: i64,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_header_drops_reserved_prefix() {
        let m = BusMessage::new("t", serde_json::json!({}))
            .with_header("x-trace-id", "abc")
            .with_header("acteon.forgery", "boom");
        assert_eq!(m.headers.get("x-trace-id").unwrap(), "abc");
        assert!(!m.headers.contains_key("acteon.forgery"));
    }
}
