//! End-to-end dispatch tests.
//!
//! These tests verify basic action dispatch through running server nodes.

use acteon_core::Action;
use acteon_simulation::prelude::*;

fn test_action(provider: &str) -> Action {
    Action::new(
        "test-ns",
        "test-tenant",
        provider,
        "test-action",
        serde_json::json!({"key": "value"}),
    )
}

#[tokio::test]
async fn single_action_dispatch() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    let action = test_action("email");
    let outcome = harness.dispatch(&action).await.expect("dispatch should succeed");

    outcome.assert_executed();
    harness.provider("email").unwrap().assert_called(1);

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn batch_dispatch() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    let actions: Vec<Action> = (0..10).map(|_| test_action("email")).collect();
    let outcomes = harness.dispatch_batch(&actions).await;

    assert_eq!(outcomes.len(), 10);
    SideEffectAssertions::assert_all_executed(&outcomes);
    harness.provider("email").unwrap().assert_called(10);

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn provider_execution_verification() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    let action = Action::new(
        "ns",
        "tenant",
        "email",
        "send_email",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Test"
        }),
    );

    harness.dispatch(&action).await.expect("dispatch should succeed");

    let provider = harness.provider("email").unwrap();
    provider.assert_called(1);

    let last_action = provider.last_action().expect("should have recorded action");
    assert_eq!(last_action.action_type, "send_email");
    assert_eq!(last_action.payload["to"], "user@example.com");

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn multiple_providers() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .add_recording_provider("push")
            .build(),
    )
    .await
    .expect("harness should start");

    // Dispatch to each provider
    harness.dispatch(&test_action("email")).await.expect("email dispatch");
    harness.dispatch(&test_action("sms")).await.expect("sms dispatch");
    harness.dispatch(&test_action("push")).await.expect("push dispatch");
    harness.dispatch(&test_action("email")).await.expect("email dispatch 2");

    harness.provider("email").unwrap().assert_called(2);
    harness.provider("sms").unwrap().assert_called(1);
    harness.provider("push").unwrap().assert_called(1);

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn dispatch_to_unknown_provider_fails() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    let action = test_action("unknown");
    let outcome = harness.dispatch(&action).await.expect("dispatch should return outcome");

    outcome.assert_failed();

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn reset_recordings_clears_all_providers() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_recording_provider("sms")
            .build(),
    )
    .await
    .expect("harness should start");

    harness.dispatch(&test_action("email")).await.expect("dispatch");
    harness.dispatch(&test_action("sms")).await.expect("dispatch");

    harness.provider("email").unwrap().assert_called(1);
    harness.provider("sms").unwrap().assert_called(1);

    harness.reset_recordings();

    harness.provider("email").unwrap().assert_not_called();
    harness.provider("sms").unwrap().assert_not_called();

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn concurrent_dispatch() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    // Dispatch many actions concurrently
    let handles: Vec<_> = (0..50)
        .map(|_| {
            let action = test_action("email");
            let node = harness.node(0).unwrap().gateway_arc();
            tokio::spawn(async move { node.dispatch(action, None).await })
        })
        .collect();

    for handle in handles {
        let outcome = handle.await.expect("task should complete");
        outcome.expect("dispatch should succeed").assert_executed();
    }

    harness.provider("email").unwrap().assert_called(50);

    harness.teardown().await.expect("teardown should succeed");
}

#[tokio::test]
async fn action_with_metadata() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .build(),
    )
    .await
    .expect("harness should start");

    let mut metadata = acteon_core::ActionMetadata::default();
    metadata.labels.insert("priority".into(), "high".into());
    metadata.labels.insert("source".into(), "test".into());

    let action = Action::new(
        "ns",
        "tenant",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    )
    .with_metadata(metadata);

    let outcome = harness.dispatch(&action).await.expect("dispatch should succeed");
    outcome.assert_executed();

    let provider = harness.provider("email").unwrap();
    let last_action = provider.last_action().expect("should have action");
    assert_eq!(
        last_action.metadata.labels.get("priority"),
        Some(&"high".to_string())
    );

    harness.teardown().await.expect("teardown should succeed");
}
