//! End-to-end rule scenario tests.
//!
//! These tests verify that different rule types work correctly in
//! an end-to-end simulation environment.

use acteon_core::Action;
use acteon_simulation::prelude::*;

// -- Rule Fixtures --

const SUPPRESSION_RULE: &str = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
"#;

const REROUTING_RULE: &str = r#"
rules:
  - name: reroute-high-priority
    priority: 1
    condition:
      field: action.payload.priority
      eq: "high"
    action:
      type: reroute
      target_provider: sms
"#;

const DEDUPLICATION_RULE: &str = r#"
rules:
  - name: dedup-notifications
    priority: 1
    condition:
      field: action.action_type
      eq: "notify"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

const THROTTLING_RULE: &str = r#"
rules:
  - name: rate-limit-bulk
    priority: 1
    condition:
      field: action.action_type
      eq: "bulk_send"
    action:
      type: throttle
      max_count: 10
      window_seconds: 60
"#;

const MODIFY_RULE: &str = r#"
rules:
  - name: add-tracking
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: modify
      changes:
        tracking_enabled: true
        modified_by: "rule-engine"
"#;

// -- Suppression Tests --

mod suppression {
    use super::*;

    #[tokio::test]
    async fn matching_action_is_suppressed() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(SUPPRESSION_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new("ns", "tenant", "email", "spam", serde_json::json!({}));
        let outcome = harness.dispatch(&action).await.expect("dispatch");

        SideEffectAssertions::assert_suppressed(&outcome);
        SideEffectAssertions::assert_suppressed_by(&outcome, "block-spam");
        harness.provider("email").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn non_matching_action_executes() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(SUPPRESSION_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "ns",
            "tenant",
            "email",
            "legitimate_email",
            serde_json::json!({}),
        );
        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_executed();
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }
}

// -- Rerouting Tests --

mod rerouting {
    use super::*;

    #[tokio::test]
    async fn high_priority_action_is_rerouted() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_recording_provider("sms")
                .add_rule_yaml(REROUTING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "ns",
            "tenant",
            "email",
            "send_notification",
            serde_json::json!({"priority": "high", "message": "urgent"}),
        );
        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_rerouted();
        SideEffectAssertions::assert_rerouted_to(&outcome, "sms");

        // Original provider should NOT be called
        harness.provider("email").unwrap().assert_not_called();

        // Target provider should be called
        harness.provider("sms").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn normal_priority_action_uses_original_provider() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_recording_provider("sms")
                .add_rule_yaml(REROUTING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "ns",
            "tenant",
            "email",
            "send_notification",
            serde_json::json!({"priority": "low", "message": "not urgent"}),
        );
        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_executed();

        // Original provider should be called
        harness.provider("email").unwrap().assert_called(1);

        // Target provider should NOT be called
        harness.provider("sms").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }
}

// -- Deduplication Tests --

mod deduplication {
    use super::*;

    #[tokio::test]
    async fn first_action_executes() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(DEDUPLICATION_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new("ns", "tenant", "email", "notify", serde_json::json!({}))
            .with_dedup_key("unique-notification-1");

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_executed();
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_action_is_deduplicated() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(DEDUPLICATION_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action1 = Action::new("ns", "tenant", "email", "notify", serde_json::json!({}))
            .with_dedup_key("duplicate-key");
        let action2 = Action::new("ns", "tenant", "email", "notify", serde_json::json!({}))
            .with_dedup_key("duplicate-key");

        // First dispatch
        let outcome1 = harness.dispatch(&action1).await.expect("dispatch 1");
        outcome1.assert_executed();

        // Second dispatch with same dedup key
        let outcome2 = harness.dispatch(&action2).await.expect("dispatch 2");
        outcome2.assert_deduplicated();

        // Provider should only be called once
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn different_dedup_keys_both_execute() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(DEDUPLICATION_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action1 = Action::new("ns", "tenant", "email", "notify", serde_json::json!({}))
            .with_dedup_key("key-1");
        let action2 = Action::new("ns", "tenant", "email", "notify", serde_json::json!({}))
            .with_dedup_key("key-2");

        let outcome1 = harness.dispatch(&action1).await.expect("dispatch 1");
        let outcome2 = harness.dispatch(&action2).await.expect("dispatch 2");

        outcome1.assert_executed();
        outcome2.assert_executed();

        harness.provider("email").unwrap().assert_called(2);

        harness.teardown().await.unwrap();
    }
}

// -- Throttling Tests --

mod throttling {
    use super::*;

    #[tokio::test]
    async fn throttled_action_returns_retry_after() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(THROTTLING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "ns",
            "tenant",
            "email",
            "bulk_send",
            serde_json::json!({"count": 1000}),
        );

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_throttled();
        harness.provider("email").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }
}

// -- Modify Tests --

mod modify {
    use super::*;

    #[tokio::test]
    async fn payload_is_modified_before_execution() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(MODIFY_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "ns",
            "tenant",
            "email",
            "send_email",
            serde_json::json!({"to": "user@example.com"}),
        );

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_executed();

        let provider = harness.provider("email").unwrap();
        provider.assert_called(1);

        let last_action = provider.last_action().expect("should have action");

        // Original payload field should be preserved
        assert_eq!(last_action.payload["to"], "user@example.com");

        // Modified fields should be added
        assert_eq!(last_action.payload["tracking_enabled"], true);
        assert_eq!(last_action.payload["modified_by"], "rule-engine");

        harness.teardown().await.unwrap();
    }
}

// -- Combined Rules Tests --

mod combined {
    use super::*;

    const PRIORITY_RULES: &str = r#"
rules:
  - name: high-prio-suppress
    priority: 1
    condition:
      all:
        - field: action.action_type
          eq: "spam"
        - field: action.payload.priority
          eq: "high"
    action:
      type: suppress

  - name: low-prio-allow
    priority: 10
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: allow
"#;

    #[tokio::test]
    async fn higher_priority_rule_wins() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(PRIORITY_RULES)
                .build(),
        )
        .await
        .expect("harness should start");

        // High priority spam should be suppressed (rule priority 1)
        let high_priority_spam = Action::new(
            "ns",
            "tenant",
            "email",
            "spam",
            serde_json::json!({"priority": "high"}),
        );
        let outcome = harness
            .dispatch(&high_priority_spam)
            .await
            .expect("dispatch");
        outcome.assert_suppressed();

        // Low priority spam should be allowed (rule priority 10)
        let low_priority_spam = Action::new(
            "ns",
            "tenant",
            "email",
            "spam",
            serde_json::json!({"priority": "low"}),
        );
        let outcome = harness
            .dispatch(&low_priority_spam)
            .await
            .expect("dispatch");
        outcome.assert_executed();

        harness.teardown().await.unwrap();
    }
}
