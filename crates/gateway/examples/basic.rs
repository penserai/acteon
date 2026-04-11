//! Basic example: Build a Gateway with MemoryStateStore, YAML rules, and a mock provider.
//!
//! Run with: `cargo run -p acteon-gateway --example basic`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;

/// A simple mock provider that always succeeds.
struct MockEmailProvider;

#[async_trait]
impl DynProvider for MockEmailProvider {
    fn name(&self) -> &str {
        "email"
    }

    async fn execute(
        &self,
        action: &Action,
    ) -> Result<acteon_core::ProviderResponse, ProviderError> {
        let recipient = action
            .payload
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        tracing::info!(
            provider = "email",
            action_type = %action.action_type,
            recipient,
            "Executing action"
        );
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"sent": true}),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Load rules from YAML
    let frontend = YamlFrontend;
    let rules_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("yaml_rules")
        .join("basic.yaml");
    let rules = frontend
        .parse_file(&rules_path)
        .expect("failed to parse rules");

    tracing::info!(count = rules.len(), "Loaded rules");
    for rule in &rules {
        tracing::info!(name = %rule.name, priority = rule.priority, "  Rule");
    }

    // Build the gateway
    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(rules)
        .provider(Arc::new(MockEmailProvider))
        .executor_config(ExecutorConfig {
            max_retries: 1,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .build()
        .expect("failed to build gateway");

    // Scenario 1: Normal email - should be deduplicated on second send
    tracing::info!("=== Scenario 1: Send email (deduplicated on repeat) ===");
    let email_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Hello!"}),
    )
    .with_dedup_key("email-user@example.com-hello");

    let outcome1 = gateway.dispatch(email_action.clone(), None).await.unwrap();
    tracing::info!(outcome = outcome_label(&outcome1), "First dispatch");

    let outcome2 = gateway.dispatch(email_action, None).await.unwrap();
    tracing::info!(outcome = outcome_label(&outcome2), "Second dispatch");

    // Scenario 2: Spam action - should be suppressed
    tracing::info!("=== Scenario 2: Spam action (suppressed) ===");
    let spam_action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "spam",
        serde_json::json!({"to": "victim@example.com"}),
    );

    let outcome3 = gateway.dispatch(spam_action, None).await.unwrap();
    tracing::info!(outcome = outcome_label(&outcome3), "Result");

    // Print metrics
    let snap = gateway.metrics().snapshot();
    tracing::info!(
        dispatched = snap.dispatched,
        executed = snap.executed,
        deduplicated = snap.deduplicated,
        suppressed = snap.suppressed,
        "Gateway metrics"
    );
}

fn outcome_label(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "Executed",
        ActionOutcome::Deduplicated => "Deduplicated",
        ActionOutcome::Suppressed { .. } => "Suppressed",
        ActionOutcome::Rerouted { .. } => "Rerouted",
        ActionOutcome::Throttled { .. } => "Throttled",
        ActionOutcome::Grouped { .. } => "Grouped",
        ActionOutcome::StateChanged { .. } => "StateChanged",
        ActionOutcome::Failed(_) => "Failed",
        ActionOutcome::PendingApproval { .. } => "PendingApproval",
        ActionOutcome::ChainStarted { .. } => "ChainStarted",
        ActionOutcome::DryRun { .. } => "DryRun",
        ActionOutcome::CircuitOpen { .. } => "CircuitOpen",
        ActionOutcome::Scheduled { .. } => "Scheduled",
        ActionOutcome::RecurringCreated { .. } => "RecurringCreated",
        ActionOutcome::QuotaExceeded { .. } => "QuotaExceeded",
        ActionOutcome::Silenced { .. } => "Silenced",
    }
}
