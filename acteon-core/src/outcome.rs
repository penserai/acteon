use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Outcome of dispatching an action through the gateway pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ActionOutcome {
    /// Action was executed by the provider.
    Executed(ProviderResponse),
    /// Action was deduplicated (already processed).
    Deduplicated,
    /// Action was suppressed by a rule.
    Suppressed { rule: String },
    /// Action was rerouted to a different provider.
    Rerouted {
        original_provider: String,
        new_provider: String,
        response: ProviderResponse,
    },
    /// Action was throttled — caller should retry later.
    Throttled {
        #[cfg_attr(feature = "openapi", schema(value_type = Object, example = json!({"secs": 60, "nanos": 0})))]
        retry_after: Duration,
    },
    /// Action failed after all retries.
    Failed(ActionError),
    /// Action was grouped for batched notification.
    Grouped {
        /// Unique identifier for the group.
        group_id: String,
        /// Current number of events in the group.
        group_size: usize,
        /// When the group will be flushed/notified.
        notify_at: DateTime<Utc>,
    },
    /// Action triggered a state machine transition.
    StateChanged {
        /// Fingerprint of the event whose state changed.
        fingerprint: String,
        /// Previous state before transition.
        previous_state: String,
        /// New state after transition.
        new_state: String,
        /// Whether this transition triggers a notification.
        notify: bool,
    },
}

/// Response from a provider after executing an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProviderResponse {
    /// Status of the execution.
    pub status: ResponseStatus,
    /// Provider-specific response body.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub body: serde_json::Value,
    /// Optional headers or metadata from the provider.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl ProviderResponse {
    /// Create a successful provider response.
    #[must_use]
    pub fn success(body: serde_json::Value) -> Self {
        Self {
            status: ResponseStatus::Success,
            body,
            headers: HashMap::new(),
        }
    }

    /// Create a failed provider response.
    #[must_use]
    pub fn failure(body: serde_json::Value) -> Self {
        Self {
            status: ResponseStatus::Failure,
            body,
            headers: HashMap::new(),
        }
    }
}

/// Status of a provider execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Success,
    Failure,
    Partial,
}

/// Error detail when an action fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionError {
    /// Error code or category.
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Whether the error is retryable.
    pub retryable: bool,
    /// Number of retry attempts made.
    pub attempts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_response_success() {
        let resp = ProviderResponse::success(serde_json::json!({"id": 42}));
        assert_eq!(resp.status, ResponseStatus::Success);
    }

    #[test]
    fn outcome_serde_roundtrip() {
        let outcome = ActionOutcome::Executed(ProviderResponse::success(serde_json::Value::Null));
        let json = serde_json::to_string(&outcome).unwrap();
        let _back: ActionOutcome = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn outcome_deduplicated() {
        let outcome = ActionOutcome::Deduplicated;
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("Deduplicated"));
    }

    #[test]
    fn outcome_suppressed() {
        let outcome = ActionOutcome::Suppressed {
            rule: "block-spam".into(),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("block-spam"));
    }

    #[test]
    fn outcome_grouped() {
        let outcome = ActionOutcome::Grouped {
            group_id: "group-123".into(),
            group_size: 5,
            notify_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("group-123"));
        assert!(json.contains("group_size"));
    }

    #[test]
    fn outcome_state_changed() {
        let outcome = ActionOutcome::StateChanged {
            fingerprint: "fp-456".into(),
            previous_state: "open".into(),
            new_state: "in_progress".into(),
            notify: true,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("fp-456"));
        assert!(json.contains("open"));
        assert!(json.contains("in_progress"));
    }
}
