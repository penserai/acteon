use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::outcome::{ActionOutcome, ProviderResponse};

/// A real-time event emitted by the gateway for SSE streaming.
///
/// Each event carries enough metadata for client-side filtering by
/// namespace, tenant, action type, and outcome category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Unique event identifier (UUID v4). Used as the SSE `id` field
    /// to support `Last-Event-ID` reconnection.
    pub id: String,
    /// When the event was emitted.
    pub timestamp: DateTime<Utc>,
    /// The specific event payload.
    #[serde(flatten)]
    pub event_type: StreamEventType,
    /// Namespace of the originating action or background task.
    pub namespace: String,
    /// Tenant of the originating action or background task.
    pub tenant: String,
    /// Action type discriminator, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Action ID, when the event originates from a dispatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
}

/// The type-specific payload of a [`StreamEvent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEventType {
    /// An action was dispatched through the gateway pipeline.
    ActionDispatched {
        /// The outcome of the dispatch.
        outcome: ActionOutcome,
        /// The target provider.
        provider: String,
    },
    /// An event group was flushed (batched notification sent).
    GroupFlushed {
        /// The group identifier.
        group_id: String,
        /// Number of events in the flushed group.
        event_count: usize,
    },
    /// A state machine timeout fired.
    Timeout {
        /// Fingerprint of the timed-out event.
        fingerprint: String,
        /// Name of the state machine.
        state_machine: String,
        /// State before the timeout transition.
        previous_state: String,
        /// State after the timeout transition.
        new_state: String,
    },
    /// A task chain step was advanced.
    ChainAdvanced {
        /// The chain execution ID.
        chain_id: String,
    },
    /// An action requires human approval.
    ApprovalRequired {
        /// The approval request ID.
        approval_id: String,
    },
}

/// Sanitize an [`ActionOutcome`] for safe inclusion in SSE stream events.
///
/// Strips sensitive fields that should not be broadcast to subscribers:
/// - `ProviderResponse.body` is replaced with `null` (may contain PII or secrets)
/// - `ProviderResponse.headers` are cleared (may contain auth tokens)
/// - `PendingApproval` URLs are redacted (HMAC-signed tokens that grant approval power)
#[must_use]
pub fn sanitize_outcome(outcome: &ActionOutcome) -> ActionOutcome {
    match outcome {
        ActionOutcome::Executed(resp) => ActionOutcome::Executed(ProviderResponse {
            status: resp.status.clone(),
            body: serde_json::Value::Null,
            headers: HashMap::new(),
        }),
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            response,
        } => ActionOutcome::Rerouted {
            original_provider: original_provider.clone(),
            new_provider: new_provider.clone(),
            response: ProviderResponse {
                status: response.status.clone(),
                body: serde_json::Value::Null,
                headers: HashMap::new(),
            },
        },
        ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            notification_sent,
            ..
        } => ActionOutcome::PendingApproval {
            approval_id: approval_id.clone(),
            expires_at: *expires_at,
            approve_url: "[redacted]".into(),
            reject_url: "[redacted]".into(),
            notification_sent: *notification_sent,
        },
        other => other.clone(),
    }
}

/// Outcome category string for filtering. Derived from [`ActionOutcome`].
///
/// Returns a short lowercase label suitable for query-parameter filtering.
#[must_use]
pub fn outcome_category(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "executed",
        ActionOutcome::Deduplicated => "deduplicated",
        ActionOutcome::Suppressed { .. } => "suppressed",
        ActionOutcome::Rerouted { .. } => "rerouted",
        ActionOutcome::Throttled { .. } => "throttled",
        ActionOutcome::Failed(_) => "failed",
        ActionOutcome::Grouped { .. } => "grouped",
        ActionOutcome::StateChanged { .. } => "state_changed",
        ActionOutcome::PendingApproval { .. } => "pending_approval",
        ActionOutcome::ChainStarted { .. } => "chain_started",
        ActionOutcome::DryRun { .. } => "dry_run",
        ActionOutcome::CircuitOpen { .. } => "circuit_open",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::outcome::{ActionError, ProviderResponse, ResponseStatus};

    // -- Helper --------------------------------------------------------------

    fn make_event(event_type: StreamEventType) -> StreamEvent {
        StreamEvent {
            id: "evt-1".into(),
            timestamp: Utc::now(),
            event_type,
            namespace: "ns".into(),
            tenant: "t1".into(),
            action_type: None,
            action_id: None,
        }
    }

    // -- Serde roundtrip tests -----------------------------------------------

    #[test]
    fn stream_event_serde_roundtrip() {
        let event = StreamEvent {
            id: "test-id".into(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ActionDispatched {
                outcome: ActionOutcome::Executed(ProviderResponse::success(
                    serde_json::json!({"ok": true}),
                )),
                provider: "email".into(),
            },
            namespace: "notifications".into(),
            tenant: "tenant-1".into(),
            action_type: Some("send_email".into()),
            action_id: Some("action-123".into()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "test-id");
        assert_eq!(back.namespace, "notifications");
        assert_eq!(back.tenant, "tenant-1");
        assert_eq!(back.action_type.as_deref(), Some("send_email"));
        assert_eq!(back.action_id.as_deref(), Some("action-123"));
        assert!(matches!(
            back.event_type,
            StreamEventType::ActionDispatched { .. }
        ));
    }

    #[test]
    fn stream_event_group_flushed() {
        let event = StreamEvent {
            id: "flush-1".into(),
            timestamp: Utc::now(),
            event_type: StreamEventType::GroupFlushed {
                group_id: "grp-abc".into(),
                event_count: 5,
            },
            namespace: "alerts".into(),
            tenant: "t1".into(),
            action_type: None,
            action_id: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("group_flushed"));
        assert!(json.contains("grp-abc"));
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::GroupFlushed {
                group_id,
                event_count,
            } => {
                assert_eq!(group_id, "grp-abc");
                assert_eq!(event_count, 5);
            }
            other => panic!("expected GroupFlushed, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_timeout() {
        let event = StreamEvent {
            id: "timeout-1".into(),
            timestamp: Utc::now(),
            event_type: StreamEventType::Timeout {
                fingerprint: "fp-123".into(),
                state_machine: "alert_sm".into(),
                previous_state: "open".into(),
                new_state: "closed".into(),
            },
            namespace: "alerts".into(),
            tenant: "t1".into(),
            action_type: None,
            action_id: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("timeout"));
        assert!(json.contains("fp-123"));
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::Timeout {
                fingerprint,
                state_machine,
                previous_state,
                new_state,
            } => {
                assert_eq!(fingerprint, "fp-123");
                assert_eq!(state_machine, "alert_sm");
                assert_eq!(previous_state, "open");
                assert_eq!(new_state, "closed");
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_chain_advanced_roundtrip() {
        let event = make_event(StreamEventType::ChainAdvanced {
            chain_id: "chain-42".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("chain_advanced"));
        assert!(json.contains("chain-42"));
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ChainAdvanced { chain_id } => {
                assert_eq!(chain_id, "chain-42");
            }
            other => panic!("expected ChainAdvanced, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_approval_required_roundtrip() {
        let event = make_event(StreamEventType::ApprovalRequired {
            approval_id: "appr-xyz".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("approval_required"));
        assert!(json.contains("appr-xyz"));
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ApprovalRequired { approval_id } => {
                assert_eq!(approval_id, "appr-xyz");
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
    }

    // -- Optional fields (skip_serializing_if = None) -------------------------

    #[test]
    fn optional_fields_omitted_when_none() {
        let event = make_event(StreamEventType::GroupFlushed {
            group_id: "g".into(),
            event_count: 1,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("action_type"),
            "action_type should be omitted when None"
        );
        assert!(
            !json.contains("action_id"),
            "action_id should be omitted when None"
        );
    }

    #[test]
    fn optional_fields_present_when_some() {
        let event = StreamEvent {
            id: "e".into(),
            timestamp: Utc::now(),
            event_type: StreamEventType::ActionDispatched {
                outcome: ActionOutcome::Deduplicated,
                provider: "p".into(),
            },
            namespace: "ns".into(),
            tenant: "t".into(),
            action_type: Some("send_email".into()),
            action_id: Some("act-1".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("send_email"));
        assert!(json.contains("act-1"));
    }

    // -- ActionDispatched with various ActionOutcome variants -----------------

    #[test]
    fn dispatched_with_suppressed_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Suppressed {
                rule: "block-spam".into(),
            },
            provider: "email".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Suppressed"));
        assert!(json.contains("block-spam"));
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, provider } => {
                assert_eq!(provider, "email");
                assert!(matches!(outcome, ActionOutcome::Suppressed { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[test]
    fn dispatched_with_throttled_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Throttled {
                retry_after: Duration::from_secs(60),
            },
            provider: "webhook".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Throttled { .. }));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[test]
    fn dispatched_with_rerouted_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Rerouted {
                original_provider: "email".into(),
                new_provider: "sms".into(),
                response: ProviderResponse::success(serde_json::json!({})),
            },
            provider: "email".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => match outcome {
                ActionOutcome::Rerouted {
                    original_provider,
                    new_provider,
                    ..
                } => {
                    assert_eq!(original_provider, "email");
                    assert_eq!(new_provider, "sms");
                }
                other => panic!("expected Rerouted, got {other:?}"),
            },
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[test]
    fn dispatched_with_failed_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Failed(ActionError {
                code: "TIMEOUT".into(),
                message: "timed out".into(),
                retryable: true,
                attempts: 3,
            }),
            provider: "webhook".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => match outcome {
                ActionOutcome::Failed(err) => {
                    assert_eq!(err.code, "TIMEOUT");
                    assert_eq!(err.attempts, 3);
                    assert!(err.retryable);
                }
                other => panic!("expected Failed, got {other:?}"),
            },
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[test]
    fn dispatched_with_deduplicated_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Deduplicated,
            provider: "email".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => {
                assert!(matches!(outcome, ActionOutcome::Deduplicated));
            }
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    #[test]
    fn dispatched_with_circuit_open_outcome() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::CircuitOpen {
                provider: "email".into(),
                fallback_chain: vec!["sms".into()],
            },
            provider: "email".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => match outcome {
                ActionOutcome::CircuitOpen {
                    provider,
                    fallback_chain,
                } => {
                    assert_eq!(provider, "email");
                    assert_eq!(fallback_chain, vec!["sms"]);
                }
                other => panic!("expected CircuitOpen, got {other:?}"),
            },
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    // -- outcome_category exhaustive tests ------------------------------------

    #[test]
    fn outcome_category_labels() {
        assert_eq!(
            outcome_category(&ActionOutcome::Executed(ProviderResponse::success(
                serde_json::Value::Null
            ))),
            "executed"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Deduplicated),
            "deduplicated"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Suppressed { rule: "r".into() }),
            "suppressed"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Rerouted {
                original_provider: "a".into(),
                new_provider: "b".into(),
                response: ProviderResponse::success(serde_json::Value::Null),
            }),
            "rerouted"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Throttled {
                retry_after: Duration::from_secs(1),
            }),
            "throttled"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Failed(ActionError {
                code: "e".into(),
                message: "m".into(),
                retryable: false,
                attempts: 1,
            })),
            "failed"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::Grouped {
                group_id: "g".into(),
                group_size: 1,
                notify_at: Utc::now(),
            }),
            "grouped"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::StateChanged {
                fingerprint: "fp".into(),
                previous_state: "a".into(),
                new_state: "b".into(),
                notify: false,
            }),
            "state_changed"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::PendingApproval {
                approval_id: "id".into(),
                expires_at: Utc::now(),
                approve_url: "u".into(),
                reject_url: "u".into(),
                notification_sent: false,
            }),
            "pending_approval"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::ChainStarted {
                chain_id: "c".into(),
                chain_name: "n".into(),
                total_steps: 1,
                first_step: "s".into(),
            }),
            "chain_started"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::DryRun {
                verdict: "allow".into(),
                matched_rule: None,
                would_be_provider: "p".into(),
            }),
            "dry_run"
        );
        assert_eq!(
            outcome_category(&ActionOutcome::CircuitOpen {
                provider: "p".into(),
                fallback_chain: vec![]
            }),
            "circuit_open"
        );
    }

    // -- Executed variant status roundtrip ------------------------------------

    #[test]
    fn dispatched_with_failure_status_response() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Executed(ProviderResponse::failure(
                serde_json::json!({"error": "bad request"}),
            )),
            provider: "webhook".into(),
        });
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        match back.event_type {
            StreamEventType::ActionDispatched { outcome, .. } => match outcome {
                ActionOutcome::Executed(resp) => {
                    assert_eq!(resp.status, ResponseStatus::Failure);
                }
                other => panic!("expected Executed, got {other:?}"),
            },
            other => panic!("expected ActionDispatched, got {other:?}"),
        }
    }

    // -- StreamEvent type tag in JSON -----------------------------------------

    #[test]
    fn type_tag_is_snake_case() {
        let cases: Vec<(StreamEventType, &str)> = vec![
            (
                StreamEventType::ActionDispatched {
                    outcome: ActionOutcome::Deduplicated,
                    provider: "p".into(),
                },
                "action_dispatched",
            ),
            (
                StreamEventType::GroupFlushed {
                    group_id: "g".into(),
                    event_count: 0,
                },
                "group_flushed",
            ),
            (
                StreamEventType::Timeout {
                    fingerprint: "f".into(),
                    state_machine: "s".into(),
                    previous_state: "a".into(),
                    new_state: "b".into(),
                },
                "timeout",
            ),
            (
                StreamEventType::ChainAdvanced {
                    chain_id: "c".into(),
                },
                "chain_advanced",
            ),
            (
                StreamEventType::ApprovalRequired {
                    approval_id: "a".into(),
                },
                "approval_required",
            ),
        ];
        for (event_type, expected_tag) in cases {
            let event = make_event(event_type);
            let json = serde_json::to_string(&event).unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                value["type"].as_str().unwrap(),
                expected_tag,
                "type tag mismatch for {expected_tag}"
            );
        }
    }

    // -- Clone ----------------------------------------------------------------

    #[test]
    fn stream_event_is_cloneable() {
        let event = make_event(StreamEventType::ActionDispatched {
            outcome: ActionOutcome::Deduplicated,
            provider: "email".into(),
        });
        let cloned = event.clone();
        assert_eq!(cloned.id, event.id);
        assert_eq!(cloned.namespace, event.namespace);
    }

    // -- sanitize_outcome security tests --------------------------------------

    #[test]
    fn sanitize_strips_provider_response_body() {
        let outcome = ActionOutcome::Executed(ProviderResponse {
            status: ResponseStatus::Success,
            body: serde_json::json!({"secret_key": "sk-12345", "pii": "user@example.com"}),
            headers: HashMap::from([("Authorization".into(), "Bearer tok".into())]),
        });
        let sanitized = sanitize_outcome(&outcome);
        match sanitized {
            ActionOutcome::Executed(resp) => {
                assert_eq!(resp.body, serde_json::Value::Null);
                assert!(resp.headers.is_empty());
                assert_eq!(resp.status, ResponseStatus::Success);
            }
            other => panic!("expected Executed, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_strips_rerouted_response_body() {
        let outcome = ActionOutcome::Rerouted {
            original_provider: "email".into(),
            new_provider: "sms".into(),
            response: ProviderResponse {
                status: ResponseStatus::Success,
                body: serde_json::json!({"internal_id": "secret-123"}),
                headers: HashMap::from([("X-Internal".into(), "val".into())]),
            },
        };
        let sanitized = sanitize_outcome(&outcome);
        match sanitized {
            ActionOutcome::Rerouted {
                original_provider,
                new_provider,
                response,
            } => {
                assert_eq!(original_provider, "email");
                assert_eq!(new_provider, "sms");
                assert_eq!(response.body, serde_json::Value::Null);
                assert!(response.headers.is_empty());
            }
            other => panic!("expected Rerouted, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_redacts_approval_urls() {
        let outcome = ActionOutcome::PendingApproval {
            approval_id: "appr-123".into(),
            expires_at: Utc::now(),
            approve_url: "https://example.com/approve?sig=hmac_secret_token".into(),
            reject_url: "https://example.com/reject?sig=hmac_secret_token".into(),
            notification_sent: true,
        };
        let sanitized = sanitize_outcome(&outcome);
        match sanitized {
            ActionOutcome::PendingApproval {
                approval_id,
                approve_url,
                reject_url,
                notification_sent,
                ..
            } => {
                assert_eq!(approval_id, "appr-123");
                assert_eq!(approve_url, "[redacted]");
                assert_eq!(reject_url, "[redacted]");
                assert!(notification_sent);
            }
            other => panic!("expected PendingApproval, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_passes_through_safe_variants() {
        // Deduplicated, Suppressed, Throttled, etc. have no sensitive data.
        let outcome = ActionOutcome::Suppressed {
            rule: "block-spam".into(),
        };
        let sanitized = sanitize_outcome(&outcome);
        match sanitized {
            ActionOutcome::Suppressed { rule } => {
                assert_eq!(rule, "block-spam");
            }
            other => panic!("expected Suppressed, got {other:?}"),
        }
    }
}
