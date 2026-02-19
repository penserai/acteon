use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{ActionId, Namespace, ProviderId, TenantId};

/// Metadata attached to an action for routing and observability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionMetadata {
    /// Arbitrary key-value pairs.
    #[serde(flatten)]
    #[cfg_attr(feature = "openapi", schema(value_type = HashMap<String, String>))]
    pub labels: HashMap<String, String>,
}

/// An action to be dispatched through the gateway pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(example = json!({
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com", "subject": "Hello"},
    "metadata": {},
    "dedup_key": null,
    "status": null,
    "fingerprint": null,
    "starts_at": null,
    "ends_at": null,
    "created_at": "2025-01-01T00:00:00Z"
})))]
pub struct Action {
    /// Unique action identifier.
    pub id: ActionId,

    /// Logical namespace grouping.
    pub namespace: Namespace,

    /// Tenant that owns this action.
    pub tenant: TenantId,

    /// Target provider for execution.
    pub provider: ProviderId,

    /// Action type discriminator (e.g. `send_email`, `send_sms`).
    pub action_type: String,

    /// Arbitrary JSON payload for the provider.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub payload: serde_json::Value,

    /// Optional metadata labels.
    #[serde(default)]
    pub metadata: ActionMetadata,

    /// Optional deduplication key. If set, duplicate actions with the same
    /// key are suppressed.
    pub dedup_key: Option<String>,

    /// Current state in the state machine (e.g., "open", "closed").
    /// State names are defined by configuration, not hardcoded.
    pub status: Option<String>,

    /// Fingerprint for correlating related events.
    /// Used to group events and track state across related actions.
    pub fingerprint: Option<String>,

    /// When this event started.
    pub starts_at: Option<DateTime<Utc>>,

    /// When this event ended.
    pub ends_at: Option<DateTime<Utc>>,

    /// Timestamp when the action was created.
    pub created_at: DateTime<Utc>,

    /// W3C Trace Context headers (`traceparent`, `tracestate`) captured at
    /// dispatch time. Carries the distributed trace identity across async
    /// boundaries such as chains, grouped notifications, and DLQ replays.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = HashMap<String, String>))]
    pub trace_context: HashMap<String, String>,

    /// Optional template profile name. When set, the gateway renders the
    /// matching [`TemplateProfile`](crate::template::TemplateProfile) fields
    /// using the payload as variables and merges the results into the payload
    /// before provider execution.
    #[serde(default)]
    pub template: Option<String>,
}

impl Action {
    /// Create a new action with required fields. Generates a UUID-v4 id and
    /// sets `created_at` to now.
    #[must_use]
    pub fn new(
        namespace: impl Into<Namespace>,
        tenant: impl Into<TenantId>,
        provider: impl Into<ProviderId>,
        action_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: ActionId::new(Uuid::new_v4().to_string()),
            namespace: namespace.into(),
            tenant: tenant.into(),
            provider: provider.into(),
            action_type: action_type.into(),
            payload,
            metadata: ActionMetadata::default(),
            dedup_key: None,
            status: None,
            fingerprint: None,
            starts_at: None,
            ends_at: None,
            created_at: Utc::now(),
            trace_context: HashMap::new(),
            template: None,
        }
    }

    /// Set a deduplication key.
    #[must_use]
    pub fn with_dedup_key(mut self, key: impl Into<String>) -> Self {
        self.dedup_key = Some(key.into());
        self
    }

    /// Set metadata labels.
    #[must_use]
    pub fn with_metadata(mut self, metadata: ActionMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the event status (state machine state).
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Set the fingerprint for event correlation.
    #[must_use]
    pub fn with_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.fingerprint = Some(fingerprint.into());
        self
    }

    /// Set the event start time.
    #[must_use]
    pub fn with_starts_at(mut self, starts_at: DateTime<Utc>) -> Self {
        self.starts_at = Some(starts_at);
        self
    }

    /// Set the event end time.
    #[must_use]
    pub fn with_ends_at(mut self, ends_at: DateTime<Utc>) -> Self {
        self.ends_at = Some(ends_at);
        self
    }

    /// Set the W3C Trace Context for distributed trace propagation.
    #[must_use]
    pub fn with_trace_context(mut self, ctx: HashMap<String, String>) -> Self {
        self.trace_context = ctx;
        self
    }

    /// Set the template profile name for payload rendering.
    #[must_use]
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = Some(template.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_creation() {
        let action = Action::new(
            "notifications",
            "tenant-1",
            "email",
            "send_email",
            serde_json::json!({"to": "user@example.com"}),
        );
        assert_eq!(action.namespace.as_str(), "notifications");
        assert_eq!(action.tenant.as_str(), "tenant-1");
        assert_eq!(action.provider.as_str(), "email");
        assert_eq!(action.action_type, "send_email");
        assert!(action.dedup_key.is_none());
    }

    #[test]
    fn action_with_dedup() {
        let action = Action::new("ns", "t", "p", "type", serde_json::Value::Null)
            .with_dedup_key("unique-123");
        assert_eq!(action.dedup_key.as_deref(), Some("unique-123"));
    }

    #[test]
    fn action_serde_roundtrip() {
        let action = Action::new("ns", "t", "p", "type", serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&action).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, action.id);
        assert_eq!(back.payload, action.payload);
    }

    #[test]
    fn action_with_status() {
        let action =
            Action::new("ns", "t", "p", "type", serde_json::Value::Null).with_status("open");
        assert_eq!(action.status.as_deref(), Some("open"));
    }

    #[test]
    fn action_with_fingerprint() {
        let action =
            Action::new("ns", "t", "p", "type", serde_json::Value::Null).with_fingerprint("fp-123");
        assert_eq!(action.fingerprint.as_deref(), Some("fp-123"));
    }

    #[test]
    fn action_with_lifecycle_times() {
        let now = Utc::now();
        let later = now + chrono::Duration::hours(1);
        let action = Action::new("ns", "t", "p", "type", serde_json::Value::Null)
            .with_starts_at(now)
            .with_ends_at(later);
        assert_eq!(action.starts_at, Some(now));
        assert_eq!(action.ends_at, Some(later));
    }
}
