//! Assertion helpers for verifying side effects in simulation tests.

use acteon_core::ActionOutcome;

use crate::provider::RecordingProvider;

/// Assertion helpers for verifying side effects.
pub struct SideEffectAssertions;

impl SideEffectAssertions {
    /// Assert that an outcome matches the `Executed` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Executed`.
    pub fn assert_executed(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Executed(_)),
            "expected Executed, got {outcome:?}"
        );
    }

    /// Assert that an outcome matches the `Deduplicated` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Deduplicated`.
    pub fn assert_deduplicated(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Deduplicated),
            "expected Deduplicated, got {outcome:?}"
        );
    }

    /// Assert that an outcome matches the `Suppressed` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Suppressed`.
    pub fn assert_suppressed(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Suppressed { .. }),
            "expected Suppressed, got {outcome:?}"
        );
    }

    /// Assert that an outcome is suppressed by a specific rule.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Suppressed` or the rule doesn't match.
    pub fn assert_suppressed_by(outcome: &ActionOutcome, expected_rule: &str) {
        match outcome {
            ActionOutcome::Suppressed { rule } => {
                assert_eq!(
                    rule, expected_rule,
                    "expected suppression by '{expected_rule}', got '{rule}'"
                );
            }
            _ => panic!("expected Suppressed, got {outcome:?}"),
        }
    }

    /// Assert that an outcome matches the `Rerouted` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Rerouted`.
    pub fn assert_rerouted(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Rerouted { .. }),
            "expected Rerouted, got {outcome:?}"
        );
    }

    /// Assert that an outcome is rerouted to a specific provider.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Rerouted` or the provider doesn't match.
    pub fn assert_rerouted_to(outcome: &ActionOutcome, expected_provider: &str) {
        match outcome {
            ActionOutcome::Rerouted { new_provider, .. } => {
                assert_eq!(
                    new_provider, expected_provider,
                    "expected reroute to '{expected_provider}', got '{new_provider}'"
                );
            }
            _ => panic!("expected Rerouted, got {outcome:?}"),
        }
    }

    /// Assert that an outcome matches the `Throttled` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Throttled`.
    pub fn assert_throttled(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Throttled { .. }),
            "expected Throttled, got {outcome:?}"
        );
    }

    /// Assert that an outcome matches the `Failed` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Failed`.
    pub fn assert_failed(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
    }

    /// Assert that an outcome matches the `Grouped` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Grouped`.
    pub fn assert_grouped(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::Grouped { .. }),
            "expected Grouped, got {outcome:?}"
        );
    }

    /// Assert that an outcome is grouped with a minimum number of events.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `Grouped` or the group size is less than expected.
    pub fn assert_grouped_with_min_size(outcome: &ActionOutcome, min_size: usize) {
        match outcome {
            ActionOutcome::Grouped { group_size, .. } => {
                assert!(
                    *group_size >= min_size,
                    "expected group size >= {min_size}, got {group_size}"
                );
            }
            _ => panic!("expected Grouped, got {outcome:?}"),
        }
    }

    /// Assert that an outcome matches the `StateChanged` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `StateChanged`.
    pub fn assert_state_changed(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::StateChanged { .. }),
            "expected StateChanged, got {outcome:?}"
        );
    }

    /// Assert that an outcome matches the `ChainStarted` variant.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `ChainStarted`.
    pub fn assert_chain_started(outcome: &ActionOutcome) {
        assert!(
            matches!(outcome, ActionOutcome::ChainStarted { .. }),
            "expected ChainStarted, got {outcome:?}"
        );
    }

    /// Assert that a chain was started with a specific chain name.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `ChainStarted` or the chain name doesn't match.
    pub fn assert_chain_started_with_name(outcome: &ActionOutcome, expected_name: &str) {
        match outcome {
            ActionOutcome::ChainStarted { chain_name, .. } => {
                assert_eq!(
                    chain_name, expected_name,
                    "expected chain name '{expected_name}', got '{chain_name}'"
                );
            }
            _ => panic!("expected ChainStarted, got {outcome:?}"),
        }
    }

    /// Assert that a state transition occurred to a specific state.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not `StateChanged` or the new state doesn't match.
    pub fn assert_state_changed_to(outcome: &ActionOutcome, expected_state: &str) {
        match outcome {
            ActionOutcome::StateChanged { new_state, .. } => {
                assert_eq!(
                    new_state, expected_state,
                    "expected state change to '{expected_state}', got '{new_state}'"
                );
            }
            _ => panic!("expected StateChanged, got {outcome:?}"),
        }
    }

    /// Assert that a provider was called exactly N times.
    ///
    /// # Panics
    ///
    /// Panics if the call count doesn't match.
    pub fn assert_provider_called(provider: &RecordingProvider, expected_count: usize) {
        provider.assert_called(expected_count);
    }

    /// Assert that a provider was not called.
    ///
    /// # Panics
    ///
    /// Panics if the provider was called.
    pub fn assert_provider_not_called(provider: &RecordingProvider) {
        provider.assert_not_called();
    }

    /// Assert that a provider received an action with a specific action type.
    ///
    /// # Panics
    ///
    /// Panics if the provider wasn't called or the last action type doesn't match.
    pub fn assert_provider_received_action_type(provider: &RecordingProvider, action_type: &str) {
        let last_action = provider
            .last_action()
            .expect("provider should have been called");
        assert_eq!(
            last_action.action_type, action_type,
            "expected action type '{}', got '{}'",
            action_type, last_action.action_type
        );
    }

    /// Assert that a provider received an action with a specific payload value.
    ///
    /// # Panics
    ///
    /// Panics if the provider wasn't called or the payload value doesn't match.
    pub fn assert_provider_received_payload_value(
        provider: &RecordingProvider,
        key: &str,
        expected_value: &serde_json::Value,
    ) {
        let last_action = provider
            .last_action()
            .expect("provider should have been called");

        let actual_value = last_action.payload.get(key);
        assert_eq!(
            actual_value,
            Some(expected_value),
            "expected payload['{key}'] = {expected_value:?}, got {actual_value:?}"
        );
    }

    /// Assert that all outcomes in a batch are successful executions.
    ///
    /// # Panics
    ///
    /// Panics if any outcome is not `Ok(Executed(_))`.
    pub fn assert_all_executed(outcomes: &[Result<ActionOutcome, acteon_gateway::GatewayError>]) {
        for (i, outcome) in outcomes.iter().enumerate() {
            match outcome {
                Ok(ActionOutcome::Executed(_)) => {}
                Ok(other) => panic!("outcome {i}: expected Executed, got {other:?}"),
                Err(e) => panic!("outcome {i}: expected success, got error: {e}"),
            }
        }
    }

    /// Assert that all outcomes in a batch succeeded (Ok variant).
    ///
    /// # Panics
    ///
    /// Panics if any outcome is `Err`.
    pub fn assert_all_succeeded(outcomes: &[Result<ActionOutcome, acteon_gateway::GatewayError>]) {
        for (i, outcome) in outcomes.iter().enumerate() {
            if let Err(e) = outcome {
                panic!("outcome {i}: expected success, got error: {e}");
            }
        }
    }
}

/// Trait extension for `ActionOutcome` with convenient assertion methods.
pub trait ActionOutcomeExt {
    /// Assert this outcome is `Executed`.
    fn assert_executed(&self);

    /// Assert this outcome is `Deduplicated`.
    fn assert_deduplicated(&self);

    /// Assert this outcome is `Suppressed`.
    fn assert_suppressed(&self);

    /// Assert this outcome is `Rerouted`.
    fn assert_rerouted(&self);

    /// Assert this outcome is `Throttled`.
    fn assert_throttled(&self);

    /// Assert this outcome is `Failed`.
    fn assert_failed(&self);

    /// Assert this outcome is `Grouped`.
    fn assert_grouped(&self);

    /// Assert this outcome is `StateChanged`.
    fn assert_state_changed(&self);

    /// Assert this outcome is `ChainStarted`.
    fn assert_chain_started(&self);

    /// Check if this outcome is `Executed`.
    fn is_executed(&self) -> bool;

    /// Check if this outcome is `Deduplicated`.
    fn is_deduplicated(&self) -> bool;

    /// Check if this outcome is `Suppressed`.
    fn is_suppressed(&self) -> bool;

    /// Check if this outcome is `Rerouted`.
    fn is_rerouted(&self) -> bool;

    /// Check if this outcome is `Throttled`.
    fn is_throttled(&self) -> bool;

    /// Check if this outcome is `Failed`.
    fn is_failed(&self) -> bool;

    /// Check if this outcome is `Grouped`.
    fn is_grouped(&self) -> bool;

    /// Check if this outcome is `StateChanged`.
    fn is_state_changed(&self) -> bool;

    /// Check if this outcome is `ChainStarted`.
    fn is_chain_started(&self) -> bool;
}

impl ActionOutcomeExt for ActionOutcome {
    fn assert_executed(&self) {
        SideEffectAssertions::assert_executed(self);
    }

    fn assert_deduplicated(&self) {
        SideEffectAssertions::assert_deduplicated(self);
    }

    fn assert_suppressed(&self) {
        SideEffectAssertions::assert_suppressed(self);
    }

    fn assert_rerouted(&self) {
        SideEffectAssertions::assert_rerouted(self);
    }

    fn assert_throttled(&self) {
        SideEffectAssertions::assert_throttled(self);
    }

    fn assert_failed(&self) {
        SideEffectAssertions::assert_failed(self);
    }

    fn assert_grouped(&self) {
        SideEffectAssertions::assert_grouped(self);
    }

    fn assert_state_changed(&self) {
        SideEffectAssertions::assert_state_changed(self);
    }

    fn assert_chain_started(&self) {
        SideEffectAssertions::assert_chain_started(self);
    }

    fn is_executed(&self) -> bool {
        matches!(self, ActionOutcome::Executed(_))
    }

    fn is_deduplicated(&self) -> bool {
        matches!(self, ActionOutcome::Deduplicated)
    }

    fn is_suppressed(&self) -> bool {
        matches!(self, ActionOutcome::Suppressed { .. })
    }

    fn is_rerouted(&self) -> bool {
        matches!(self, ActionOutcome::Rerouted { .. })
    }

    fn is_throttled(&self) -> bool {
        matches!(self, ActionOutcome::Throttled { .. })
    }

    fn is_failed(&self) -> bool {
        matches!(self, ActionOutcome::Failed(_))
    }

    fn is_grouped(&self) -> bool {
        matches!(self, ActionOutcome::Grouped { .. })
    }

    fn is_state_changed(&self) -> bool {
        matches!(self, ActionOutcome::StateChanged { .. })
    }

    fn is_chain_started(&self) -> bool {
        matches!(self, ActionOutcome::ChainStarted { .. })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use acteon_core::{ActionError, ProviderResponse};

    use super::*;

    #[test]
    fn assert_executed_passes() {
        let outcome = ActionOutcome::Executed(ProviderResponse::success(serde_json::json!({})));
        SideEffectAssertions::assert_executed(&outcome);
        outcome.assert_executed();
    }

    #[test]
    #[should_panic(expected = "expected Executed")]
    fn assert_executed_fails_on_suppressed() {
        let outcome = ActionOutcome::Suppressed {
            rule: "test".into(),
        };
        SideEffectAssertions::assert_executed(&outcome);
    }

    #[test]
    fn assert_deduplicated_passes() {
        let outcome = ActionOutcome::Deduplicated;
        SideEffectAssertions::assert_deduplicated(&outcome);
        outcome.assert_deduplicated();
    }

    #[test]
    fn assert_suppressed_passes() {
        let outcome = ActionOutcome::Suppressed {
            rule: "test".into(),
        };
        SideEffectAssertions::assert_suppressed(&outcome);
        outcome.assert_suppressed();
    }

    #[test]
    fn assert_suppressed_by_passes() {
        let outcome = ActionOutcome::Suppressed {
            rule: "my-rule".into(),
        };
        SideEffectAssertions::assert_suppressed_by(&outcome, "my-rule");
    }

    #[test]
    #[should_panic(expected = "expected suppression by 'other-rule'")]
    fn assert_suppressed_by_fails_on_wrong_rule() {
        let outcome = ActionOutcome::Suppressed {
            rule: "my-rule".into(),
        };
        SideEffectAssertions::assert_suppressed_by(&outcome, "other-rule");
    }

    #[test]
    fn assert_rerouted_passes() {
        let outcome = ActionOutcome::Rerouted {
            original_provider: "email".into(),
            new_provider: "sms".into(),
            response: ProviderResponse::success(serde_json::json!({})),
        };
        SideEffectAssertions::assert_rerouted(&outcome);
        outcome.assert_rerouted();
    }

    #[test]
    fn assert_rerouted_to_passes() {
        let outcome = ActionOutcome::Rerouted {
            original_provider: "email".into(),
            new_provider: "sms".into(),
            response: ProviderResponse::success(serde_json::json!({})),
        };
        SideEffectAssertions::assert_rerouted_to(&outcome, "sms");
    }

    #[test]
    fn assert_throttled_passes() {
        let outcome = ActionOutcome::Throttled {
            retry_after: Duration::from_secs(60),
        };
        SideEffectAssertions::assert_throttled(&outcome);
        outcome.assert_throttled();
    }

    #[test]
    fn assert_failed_passes() {
        let outcome = ActionOutcome::Failed(ActionError {
            code: "TEST".into(),
            message: "test error".into(),
            retryable: false,
            attempts: 1,
        });
        SideEffectAssertions::assert_failed(&outcome);
        outcome.assert_failed();
    }

    #[test]
    fn is_methods_work() {
        let executed = ActionOutcome::Executed(ProviderResponse::success(serde_json::json!({})));
        assert!(executed.is_executed());
        assert!(!executed.is_suppressed());

        let suppressed = ActionOutcome::Suppressed {
            rule: "test".into(),
        };
        assert!(suppressed.is_suppressed());
        assert!(!suppressed.is_executed());
    }

    #[test]
    fn assert_grouped_passes() {
        let outcome = ActionOutcome::Grouped {
            group_id: "group-123".into(),
            group_size: 5,
            notify_at: chrono::Utc::now(),
        };
        SideEffectAssertions::assert_grouped(&outcome);
        outcome.assert_grouped();
    }

    #[test]
    fn assert_grouped_with_min_size_passes() {
        let outcome = ActionOutcome::Grouped {
            group_id: "group-123".into(),
            group_size: 5,
            notify_at: chrono::Utc::now(),
        };
        SideEffectAssertions::assert_grouped_with_min_size(&outcome, 3);
        SideEffectAssertions::assert_grouped_with_min_size(&outcome, 5);
    }

    #[test]
    #[should_panic(expected = "expected group size >= 10")]
    fn assert_grouped_with_min_size_fails_on_small_group() {
        let outcome = ActionOutcome::Grouped {
            group_id: "group-123".into(),
            group_size: 5,
            notify_at: chrono::Utc::now(),
        };
        SideEffectAssertions::assert_grouped_with_min_size(&outcome, 10);
    }

    #[test]
    fn assert_state_changed_passes() {
        let outcome = ActionOutcome::StateChanged {
            fingerprint: "fp-123".into(),
            previous_state: "open".into(),
            new_state: "closed".into(),
            notify: true,
        };
        SideEffectAssertions::assert_state_changed(&outcome);
        outcome.assert_state_changed();
    }

    #[test]
    fn assert_state_changed_to_passes() {
        let outcome = ActionOutcome::StateChanged {
            fingerprint: "fp-123".into(),
            previous_state: "open".into(),
            new_state: "closed".into(),
            notify: true,
        };
        SideEffectAssertions::assert_state_changed_to(&outcome, "closed");
    }

    #[test]
    #[should_panic(expected = "expected state change to 'resolved'")]
    fn assert_state_changed_to_fails_on_wrong_state() {
        let outcome = ActionOutcome::StateChanged {
            fingerprint: "fp-123".into(),
            previous_state: "open".into(),
            new_state: "closed".into(),
            notify: true,
        };
        SideEffectAssertions::assert_state_changed_to(&outcome, "resolved");
    }

    #[test]
    fn is_grouped_works() {
        let grouped = ActionOutcome::Grouped {
            group_id: "group-123".into(),
            group_size: 5,
            notify_at: chrono::Utc::now(),
        };
        assert!(grouped.is_grouped());
        assert!(!grouped.is_executed());
    }

    #[test]
    fn is_state_changed_works() {
        let state_changed = ActionOutcome::StateChanged {
            fingerprint: "fp-123".into(),
            previous_state: "open".into(),
            new_state: "closed".into(),
            notify: true,
        };
        assert!(state_changed.is_state_changed());
        assert!(!state_changed.is_executed());
    }

    #[test]
    fn assert_chain_started_passes() {
        let outcome = ActionOutcome::ChainStarted {
            chain_id: "chain-123".into(),
            chain_name: "search-summarize-email".into(),
            total_steps: 3,
            first_step: "search".into(),
        };
        SideEffectAssertions::assert_chain_started(&outcome);
        outcome.assert_chain_started();
    }

    #[test]
    fn assert_chain_started_with_name_passes() {
        let outcome = ActionOutcome::ChainStarted {
            chain_id: "chain-123".into(),
            chain_name: "search-summarize-email".into(),
            total_steps: 3,
            first_step: "search".into(),
        };
        SideEffectAssertions::assert_chain_started_with_name(&outcome, "search-summarize-email");
    }

    #[test]
    #[should_panic(expected = "expected chain name 'other-chain'")]
    fn assert_chain_started_with_name_fails_on_wrong_name() {
        let outcome = ActionOutcome::ChainStarted {
            chain_id: "chain-123".into(),
            chain_name: "search-summarize-email".into(),
            total_steps: 3,
            first_step: "search".into(),
        };
        SideEffectAssertions::assert_chain_started_with_name(&outcome, "other-chain");
    }

    #[test]
    fn is_chain_started_works() {
        let chain_started = ActionOutcome::ChainStarted {
            chain_id: "chain-123".into(),
            chain_name: "search-summarize-email".into(),
            total_steps: 3,
            first_step: "search".into(),
        };
        assert!(chain_started.is_chain_started());
        assert!(!chain_started.is_executed());
    }
}
