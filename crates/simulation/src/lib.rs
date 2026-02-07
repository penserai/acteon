//! Acteon Simulation Framework
//!
//! This crate provides tools for testing Acteon deployments in a controlled
//! local environment. It supports:
//!
//! - Multi-node cluster simulation with shared or isolated state
//! - Recording providers that capture all calls for verification
//! - Failing providers that simulate various error scenarios
//! - Assertions for verifying action outcomes and side effects
//! - Backend detection for conditional test execution
//!
//! # Quick Start
//!
//! ```no_run
//! use acteon_simulation::prelude::*;
//! use acteon_core::Action;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a simple single-node harness
//!     let harness = SimulationHarness::start(
//!         SimulationConfig::builder()
//!             .nodes(1)
//!             .add_recording_provider("email")
//!             .build()
//!     ).await.unwrap();
//!
//!     // Dispatch an action
//!     let action = Action::new("ns", "tenant", "email", "send_email", serde_json::json!({}));
//!     let outcome = harness.dispatch(&action).await.unwrap();
//!
//!     // Verify the provider was called
//!     harness.provider("email").unwrap().assert_called(1);
//!
//!     // Clean up
//!     harness.teardown().await.unwrap();
//! }
//! ```
//!
//! # Testing Rules
//!
//! ```no_run
//! use acteon_simulation::prelude::*;
//! use acteon_core::Action;
//!
//! const SUPPRESSION_RULE: &str = r#"
//! rules:
//!   - name: block-spam
//!     priority: 1
//!     condition:
//!       field: action.action_type
//!       eq: "spam"
//!     action:
//!       type: suppress
//! "#;
//!
//! #[tokio::main]
//! async fn main() {
//!     let harness = SimulationHarness::start(
//!         SimulationConfig::builder()
//!             .nodes(1)
//!             .add_recording_provider("email")
//!             .add_rule_yaml(SUPPRESSION_RULE)
//!             .build()
//!     ).await.unwrap();
//!
//!     // This action should be suppressed
//!     let action = Action::new("ns", "tenant", "email", "spam", serde_json::json!({}));
//!     let outcome = harness.dispatch(&action).await.unwrap();
//!
//!     // Verify suppression
//!     outcome.assert_suppressed();
//!     harness.provider("email").unwrap().assert_not_called();
//!
//!     harness.teardown().await.unwrap();
//! }
//! ```
//!
//! # Multi-Node Testing
//!
//! ```no_run
//! use acteon_simulation::prelude::*;
//! use acteon_core::Action;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a 3-node cluster with shared state
//!     let harness = SimulationHarness::multi_node_memory(3).await.unwrap();
//!
//!     // Dispatch to different nodes
//!     let action = Action::new("ns", "tenant", "test", "action", serde_json::json!({}));
//!     harness.dispatch_to(0, &action).await.unwrap();
//!     harness.dispatch_to(1, &action).await.unwrap();
//!     harness.dispatch_to(2, &action).await.unwrap();
//!
//!     harness.teardown().await.unwrap();
//! }
//! ```

pub mod assertions;
pub mod backend_detector;
pub mod cluster;
mod error;
pub mod harness;
pub mod provider;

pub use assertions::{ActionOutcomeExt, SideEffectAssertions};
pub use backend_detector::AvailableBackends;
pub use cluster::{
    AuditBackendConfig, ClusterConfig, PortAllocator, ServerNode, SimulationConfig,
    StateBackendConfig,
};
pub use error::SimulationError;
pub use harness::{SimulationHarness, SimulationHarnessBuilder};
pub use provider::{CapturedCall, FailingProvider, FailureMode, FailureType, RecordingProvider};

// Re-export acteon-client types for convenience
pub use acteon_client::{
    ActeonClient, ActeonClientBuilder, AuditPage, AuditQuery, AuditRecord, BatchResult,
    ReloadResult, ReplayQuery, ReplayResult, ReplaySummary, RuleInfo,
};

/// Prelude module for convenient imports.
///
/// ```
/// use acteon_simulation::prelude::*;
/// ```
pub mod prelude {
    pub use crate::assertions::{ActionOutcomeExt, SideEffectAssertions};
    pub use crate::backend_detector::AvailableBackends;
    pub use crate::cluster::{
        AuditBackendConfig, PortAllocator, ServerNode, SimulationConfig, StateBackendConfig,
    };
    pub use crate::error::SimulationError;
    pub use crate::harness::{SimulationHarness, SimulationHarnessBuilder};
    pub use crate::provider::{
        CapturedCall, FailingProvider, FailureMode, FailureType, RecordingProvider,
    };
    pub use crate::{skip_without_postgres, skip_without_redis};
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::Action;

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
    async fn integration_basic_dispatch() {
        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .build(),
        )
        .await
        .unwrap();

        let action = test_action("email");
        let outcome = harness.dispatch(&action).await.unwrap();

        assert!(outcome.is_executed());
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn integration_suppression_rule() {
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

        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(SUPPRESSION_RULE)
                .build(),
        )
        .await
        .unwrap();

        // Action that should be suppressed
        let spam_action = Action::new("ns", "tenant", "email", "spam", serde_json::json!({}));
        let outcome = harness.dispatch(&spam_action).await.unwrap();

        outcome.assert_suppressed();
        harness.provider("email").unwrap().assert_not_called();

        // Action that should execute
        let normal_action =
            Action::new("ns", "tenant", "email", "send_email", serde_json::json!({}));
        let outcome = harness.dispatch(&normal_action).await.unwrap();

        outcome.assert_executed();
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }

    #[tokio::test]
    async fn integration_deduplication() {
        const DEDUP_RULE: &str = r#"
rules:
  - name: dedup-email
    priority: 1
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: deduplicate
      ttl_seconds: 300
"#;

        let harness = SimulationHarness::start(
            SimulationConfig::builder()
                .nodes(1)
                .add_recording_provider("email")
                .add_rule_yaml(DEDUP_RULE)
                .build(),
        )
        .await
        .unwrap();

        // First action should execute
        let action1 = Action::new("ns", "tenant", "email", "send_email", serde_json::json!({}))
            .with_dedup_key("unique-123");
        let outcome1 = harness.dispatch(&action1).await.unwrap();
        outcome1.assert_executed();

        // Second action with same dedup key should be deduplicated
        let action2 = Action::new("ns", "tenant", "email", "send_email", serde_json::json!({}))
            .with_dedup_key("unique-123");
        let outcome2 = harness.dispatch(&action2).await.unwrap();
        outcome2.assert_deduplicated();

        // Provider should only have been called once
        harness.provider("email").unwrap().assert_called(1);

        harness.teardown().await.unwrap();
    }
}
