//! Multi-provider example: Multiple providers with rerouting and throttling.
//!
//! Run with: `cargo run -p acteon-gateway --example multi_provider`

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

struct MockProvider {
    provider_name: String,
}

impl MockProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_owned(),
        }
    }
}

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn execute(
        &self,
        action: &Action,
    ) -> Result<acteon_core::ProviderResponse, ProviderError> {
        println!(
            "  [{}-provider] Executing '{}' action",
            self.provider_name, action.action_type
        );
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"provider": self.provider_name, "sent": true}),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let frontend = YamlFrontend;
    let rules_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("yaml_rules")
        .join("multi_provider.yaml");
    let rules = frontend
        .parse_file(&rules_path)
        .expect("failed to parse rules");

    println!("Loaded {} rules:", rules.len());
    for rule in &rules {
        println!(
            "  - {} (priority: {}, action: {})",
            rule.name,
            rule.priority,
            rule.action.kind_label()
        );
    }
    println!();

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(rules)
        .provider(Arc::new(MockProvider::new("email")))
        .provider(Arc::new(MockProvider::new("sms")))
        .provider(Arc::new(MockProvider::new("webhook")))
        .executor_config(ExecutorConfig {
            max_retries: 1,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .build()
        .expect("failed to build gateway");

    // Scenario 1: Urgent notification - rerouted to SMS
    println!("=== Scenario 1: Urgent notification (rerouted to SMS) ===");
    let urgent = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        serde_json::json!({"to": "admin@example.com", "priority": "urgent", "body": "Server down!"}),
    );
    let outcome = gateway.dispatch(urgent, None).await.unwrap();
    println!("  Result: {}", describe_outcome(&outcome));
    println!();

    // Scenario 2: Normal notification - throttled
    println!("=== Scenario 2: Normal notification (throttled) ===");
    let normal = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        serde_json::json!({"to": "user@example.com", "body": "Weekly digest"}),
    );
    let outcome = gateway.dispatch(normal, None).await.unwrap();
    println!("  Result: {}", describe_outcome(&outcome));
    println!();

    // Scenario 3: Email action - modified with tracking
    println!("=== Scenario 3: Email action (modified with tracking) ===");
    let email = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Newsletter"}),
    );
    let outcome = gateway.dispatch(email, None).await.unwrap();
    println!("  Result: {}", describe_outcome(&outcome));
    println!();

    let snap = gateway.metrics().snapshot();
    println!("=== Gateway Metrics ===");
    println!("  Dispatched:    {}", snap.dispatched);
    println!("  Executed:      {}", snap.executed);
    println!("  Rerouted:      {}", snap.rerouted);
    println!("  Throttled:     {}", snap.throttled);
}

fn describe_outcome(outcome: &ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Executed(_) => "Executed".to_string(),
        ActionOutcome::Deduplicated => "Deduplicated".to_string(),
        ActionOutcome::Suppressed { rule } => format!("Suppressed by rule '{rule}'"),
        ActionOutcome::Rerouted {
            original_provider,
            new_provider,
            ..
        } => format!("Rerouted from '{original_provider}' to '{new_provider}'"),
        ActionOutcome::Throttled { retry_after } => {
            format!("Throttled (retry after {retry_after:?})")
        }
        ActionOutcome::Grouped {
            group_id,
            group_size,
            ..
        } => format!("Grouped (id: {group_id}, size: {group_size})"),
        ActionOutcome::StateChanged {
            fingerprint,
            previous_state,
            new_state,
            ..
        } => format!("StateChanged ({fingerprint}: {previous_state} -> {new_state})"),
        ActionOutcome::Failed(err) => format!("Failed: {}", err.message),
        ActionOutcome::PendingApproval {
            approval_id,
            expires_at,
            ..
        } => format!("PendingApproval (id: {approval_id}, expires: {expires_at})"),
        ActionOutcome::ChainStarted {
            chain_id,
            chain_name,
            ..
        } => format!("ChainStarted (id: {chain_id}, chain: {chain_name})"),
    }
}
