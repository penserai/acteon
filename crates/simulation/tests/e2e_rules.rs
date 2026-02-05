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

// -- Grouping Tests --

mod grouping {
    use super::*;

    const BASIC_GROUPING_RULE: &str = r#"
rules:
  - name: group-alerts
    priority: 10
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: group
      group_by:
        - tenant
        - payload.cluster
      group_wait_seconds: 30
      group_interval_seconds: 300
      max_group_size: 100
"#;

    const NOTIFICATION_GROUPING_RULE: &str = r#"
rules:
  - name: batch-notifications
    priority: 10
    condition:
      field: action.action_type
      eq: "notification"
    action:
      type: group
      group_by:
        - payload.user_id
      group_wait_seconds: 60
      group_interval_seconds: 600
      max_group_size: 50
"#;

    #[tokio::test]
    async fn action_matching_group_rule_is_grouped() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(BASIC_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "message": "High CPU"}),
        );

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_grouped();
        // Provider should NOT be called immediately (action is grouped)
        harness.provider("slack").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn non_matching_action_executes_immediately() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(BASIC_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        // This action doesn't match the grouping rule (action_type != "alert")
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "message",
            serde_json::json!({"text": "Hello"}),
        );

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_executed();
        harness.provider("slack").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn multiple_actions_same_group_key_all_grouped() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(BASIC_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        // All three have same group key (tenant=acme, cluster=prod)
        let action1 = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "source": "service-a"}),
        );
        let action2 = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "source": "service-b"}),
        );
        let action3 = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "source": "service-c"}),
        );

        let outcome1 = harness.dispatch(&action1).await.expect("dispatch 1");
        let outcome2 = harness.dispatch(&action2).await.expect("dispatch 2");
        let outcome3 = harness.dispatch(&action3).await.expect("dispatch 3");

        outcome1.assert_grouped();
        outcome2.assert_grouped();
        outcome3.assert_grouped();

        // All should be in same group, provider not called
        harness.provider("slack").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn different_group_keys_create_separate_groups() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(BASIC_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        // Different clusters = different groups
        let action_prod = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "message": "Prod alert"}),
        );
        let action_staging = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "staging", "message": "Staging alert"}),
        );

        let outcome_prod = harness.dispatch(&action_prod).await.expect("dispatch prod");
        let outcome_staging = harness
            .dispatch(&action_staging)
            .await
            .expect("dispatch staging");

        outcome_prod.assert_grouped();
        outcome_staging.assert_grouped();

        // Both grouped but provider not called
        harness.provider("slack").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn grouping_by_payload_field() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("push")
                .add_rule_yaml(NOTIFICATION_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        // Same user = same group
        let action1 = Action::new(
            "ns",
            "tenant",
            "push",
            "notification",
            serde_json::json!({"user_id": "user-123", "message": "Msg 1"}),
        );
        let action2 = Action::new(
            "ns",
            "tenant",
            "push",
            "notification",
            serde_json::json!({"user_id": "user-123", "message": "Msg 2"}),
        );

        let outcome1 = harness.dispatch(&action1).await.expect("dispatch 1");
        let outcome2 = harness.dispatch(&action2).await.expect("dispatch 2");

        outcome1.assert_grouped();
        outcome2.assert_grouped();

        harness.provider("push").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn multi_node_grouping_with_shared_state() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(3)
                .shared_state(true)
                .add_recording_provider("slack")
                .add_rule_yaml(BASIC_GROUPING_RULE)
                .build(),
        )
        .await
        .expect("harness should start");

        // Dispatch same group key to different nodes
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "message": "Test"}),
        );

        let outcome0 = harness
            .dispatch_to(0, &action)
            .await
            .expect("dispatch to 0");
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "message": "Test"}),
        );
        let outcome1 = harness
            .dispatch_to(1, &action)
            .await
            .expect("dispatch to 1");
        let action = Action::new(
            "monitoring",
            "acme",
            "slack",
            "alert",
            serde_json::json!({"cluster": "prod", "message": "Test"}),
        );
        let outcome2 = harness
            .dispatch_to(2, &action)
            .await
            .expect("dispatch to 2");

        outcome0.assert_grouped();
        outcome1.assert_grouped();
        outcome2.assert_grouped();

        // All grouped, none executed
        harness.provider("slack").unwrap().assert_not_called();

        harness.teardown().await.unwrap();
    }
}

// -- State Machine Tests --

mod state_machine {
    use super::*;
    use acteon_core::{StateMachineConfig, TransitionConfig};

    const TICKET_STATE_MACHINE_RULE: &str = r#"
rules:
  - name: ticket-lifecycle
    priority: 5
    condition:
      field: action.action_type
      eq: "ticket"
    action:
      type: state_machine
      state_machine: ticket
      fingerprint_fields:
        - action_type
        - payload.ticket_id
"#;

    #[allow(dead_code)]
    const ALERT_STATE_MACHINE_RULE: &str = r#"
rules:
  - name: alert-lifecycle
    priority: 5
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: state_machine
      state_machine: alert
      fingerprint_fields:
        - payload.alert_id
"#;

    /// Create the "ticket" state machine configuration.
    fn ticket_state_machine() -> StateMachineConfig {
        StateMachineConfig::new("ticket", "open")
            .with_state("in_progress")
            .with_state("closed")
            .with_transition(TransitionConfig::new("open", "in_progress"))
            .with_transition(TransitionConfig::new("open", "closed"))
            .with_transition(TransitionConfig::new("in_progress", "closed"))
    }

    #[tokio::test]
    async fn state_machine_creates_initial_state() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(TICKET_STATE_MACHINE_RULE)
                .add_state_machine(ticket_state_machine())
                .build(),
        )
        .await
        .expect("harness should start");

        let action = Action::new(
            "support",
            "acme",
            "slack",
            "ticket",
            serde_json::json!({"ticket_id": "TKT-001", "subject": "Help needed"}),
        )
        .with_status("open");

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        outcome.assert_state_changed();
        SideEffectAssertions::assert_state_changed_to(&outcome, "open");

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn state_machine_transitions_state() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(TICKET_STATE_MACHINE_RULE)
                .add_state_machine(ticket_state_machine())
                .build(),
        )
        .await
        .expect("harness should start");

        // Create ticket in open state
        let action1 = Action::new(
            "support",
            "acme",
            "slack",
            "ticket",
            serde_json::json!({"ticket_id": "TKT-002", "subject": "Issue"}),
        )
        .with_status("open")
        .with_fingerprint("ticket:TKT-002");

        let outcome1 = harness.dispatch(&action1).await.expect("dispatch 1");
        outcome1.assert_state_changed();

        // Transition to in_progress
        let action2 = Action::new(
            "support",
            "acme",
            "slack",
            "ticket",
            serde_json::json!({"ticket_id": "TKT-002", "subject": "Issue"}),
        )
        .with_status("in_progress")
        .with_fingerprint("ticket:TKT-002");

        let outcome2 = harness.dispatch(&action2).await.expect("dispatch 2");
        outcome2.assert_state_changed();
        SideEffectAssertions::assert_state_changed_to(&outcome2, "in_progress");

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn non_matching_action_bypasses_state_machine() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("slack")
                .add_rule_yaml(TICKET_STATE_MACHINE_RULE)
                .add_state_machine(ticket_state_machine())
                .build(),
        )
        .await
        .expect("harness should start");

        // Not a ticket action
        let action = Action::new(
            "support",
            "acme",
            "slack",
            "message",
            serde_json::json!({"text": "Hello"}),
        );

        let outcome = harness.dispatch(&action).await.expect("dispatch");

        // Should execute normally, not trigger state machine
        outcome.assert_executed();
        harness.provider("slack").unwrap().assert_called(1);

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
