//! Demonstration of Tenant Usage Quotas in the simulation framework.
//!
//! This example shows how quota policies enforce per-tenant usage limits in
//! the gateway dispatch pipeline. Quotas are checked after lock acquisition
//! but before rule evaluation, meaning they take precedence over all rules.
//!
//! Scenarios demonstrated:
//!   1. Block behavior — tenant A is capped at 10 actions/hour; the last 5
//!      of 15 dispatches receive `ActionOutcome::QuotaExceeded`
//!   2. Warn behavior — tenant B has a 100 actions/day limit with Warn;
//!      all actions proceed but overages are tracked via metrics
//!   3. Degrade behavior — tenant C has a quota with Degrade; when exceeded
//!      the outcome reports the fallback provider for re-routing
//!
//! Run with: `cargo run -p acteon-simulation --example quota_simulation`

use std::sync::Arc;

use acteon_core::{Action, ActionOutcome, OverageBehavior, QuotaPolicy, QuotaWindow};
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;
use chrono::Utc;
use tracing::info;

// =============================================================================
// Mock providers
// =============================================================================

/// A simple mock provider that always succeeds.
struct MockProvider {
    name: &'static str,
}

impl MockProvider {
    const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn execute(
        &self,
        action: &Action,
    ) -> Result<acteon_core::ProviderResponse, ProviderError> {
        info!(
            "    [{}-provider] executed '{}' for tenant '{}'",
            self.name, action.action_type, action.tenant
        );
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"provider": self.name, "ok": true}),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║           TENANT USAGE QUOTAS SIMULATION DEMO                ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    let now = Utc::now();

    // =========================================================================
    // SCENARIO 1: Block Behavior (10 actions/hour)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: BLOCK BEHAVIOR (10 actions/hour limit)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant A has a quota of 10 actions per hour with Block behavior.");
    info!("  We dispatch 15 actions: the first 10 succeed, the last 5 are");
    info!("  rejected with QuotaExceeded.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .quota_policy(QuotaPolicy {
            id: "q-tenant-a".into(),
            namespace: "notifications".into(),
            tenant: "tenant-a".into(),
            provider: None,
            max_actions: 10,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            created_at: now,
            updated_at: now,
            description: Some("Tenant A: 10 actions/hour, block on exceed".into()),
            labels: Default::default(),
        })
        .build()?;

    info!("  Gateway built with quota: tenant-a = 10 actions/hour (Block)\n");

    let mut executed = 0u32;
    let mut blocked = 0u32;

    for i in 1..=15 {
        let action = Action::new(
            "notifications",
            "tenant-a",
            "email",
            "send_notification",
            serde_json::json!({"request_num": i, "to": format!("user-{i}@example.com")}),
        );

        let outcome = gateway.dispatch(action, None).await?;

        match &outcome {
            ActionOutcome::Executed(_) => {
                executed += 1;
                info!("  [dispatch #{i:>2}] Executed (within quota)");
            }
            ActionOutcome::QuotaExceeded {
                limit,
                used,
                overage_behavior,
                ..
            } => {
                blocked += 1;
                info!(
                    "  [dispatch #{i:>2}] QuotaExceeded — limit={limit}, used={used}, behavior={overage_behavior}"
                );
            }
            other => {
                info!("  [dispatch #{i:>2}] Unexpected: {}", outcome_label(other));
            }
        }
    }

    info!("\n  ┌─────────────────────────────────┐");
    info!("  │  Results                         │");
    info!("  ├─────────────────────────────────┤");
    info!("  │  Executed:        {executed:>3}             │");
    info!("  │  Blocked:         {blocked:>3}             │");
    info!(
        "  │  quota_exceeded:  {:>3}             │",
        gateway.metrics().snapshot().quota_exceeded
    );
    info!("  └─────────────────────────────────┘");

    assert_eq!(executed, 10, "expected 10 actions to execute");
    assert_eq!(blocked, 5, "expected 5 actions to be blocked");
    assert_eq!(gateway.metrics().snapshot().quota_exceeded, 5);

    info!("");

    // =========================================================================
    // SCENARIO 2: Warn Behavior (100 actions/day)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: WARN BEHAVIOR (100 actions/day limit)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant B has a quota of 100 actions per day with Warn behavior.");
    info!("  We send 105 actions: all proceed, but the last 5 trigger quota");
    info!("  warnings tracked via the quota_warned metric.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("webhook")))
        .quota_policy(QuotaPolicy {
            id: "q-tenant-b".into(),
            namespace: "analytics".into(),
            tenant: "tenant-b".into(),
            provider: None,
            max_actions: 100,
            window: QuotaWindow::Daily,
            overage_behavior: OverageBehavior::Warn,
            enabled: true,
            created_at: now,
            updated_at: now,
            description: Some("Tenant B: 100 actions/day, warn on exceed".into()),
            labels: Default::default(),
        })
        .build()?;

    info!("  Gateway built with quota: tenant-b = 100 actions/day (Warn)\n");

    let total_dispatches = 105u32;
    let mut all_executed = true;

    for i in 1..=total_dispatches {
        let action = Action::new(
            "analytics",
            "tenant-b",
            "webhook",
            "track_event",
            serde_json::json!({"event": "page_view", "seq": i}),
        );

        let outcome = gateway.dispatch(action, None).await?;

        if !matches!(outcome, ActionOutcome::Executed(_)) {
            all_executed = false;
            info!("  [dispatch #{i}] Unexpected: {}", outcome_label(&outcome));
        }
    }

    assert!(all_executed, "Warn behavior should allow all actions");

    let snap = gateway.metrics().snapshot();
    let over_quota = total_dispatches - 100;

    info!("  Dispatched {total_dispatches} actions (limit: 100)");
    info!("");
    info!("  ┌─────────────────────────────────┐");
    info!("  │  Results                         │");
    info!("  ├─────────────────────────────────┤");
    info!("  │  All executed:    yes             │");
    info!("  │  Over quota:      {over_quota:>3}             │");
    info!(
        "  │  quota_warned:    {:>3}             │",
        snap.quota_warned
    );
    info!(
        "  │  quota_exceeded:  {:>3}             │",
        snap.quota_exceeded
    );
    info!("  └─────────────────────────────────┘");

    assert_eq!(
        snap.quota_warned,
        u64::from(over_quota),
        "expected {over_quota} warnings for actions over the limit"
    );
    assert_eq!(snap.quota_exceeded, 0, "Warn should never block");

    info!("");

    // =========================================================================
    // SCENARIO 3: Degrade Behavior (provider swap on overage)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: DEGRADE BEHAVIOR (5 actions/hour, fallback to log)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant C has a quota of 5 actions per hour with Degrade behavior.");
    info!("  When the quota is exceeded, the gateway returns QuotaExceeded with");
    info!("  the fallback provider name so the caller can re-route the action.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("premium-sms")))
        .provider(Arc::new(MockProvider::new("log-fallback")))
        .quota_policy(QuotaPolicy {
            id: "q-tenant-c".into(),
            namespace: "messaging".into(),
            tenant: "tenant-c".into(),
            provider: None,
            max_actions: 5,
            window: QuotaWindow::Hourly,
            overage_behavior: OverageBehavior::Degrade {
                fallback_provider: "log-fallback".into(),
            },
            enabled: true,
            created_at: now,
            updated_at: now,
            description: Some("Tenant C: 5/hour, degrade to log-fallback".into()),
            labels: Default::default(),
        })
        .build()?;

    info!("  Gateway built with quota: tenant-c = 5 actions/hour (Degrade -> log-fallback)\n");

    let mut executed_premium = 0u32;
    let mut degraded = 0u32;
    let mut fallback_providers: Vec<String> = Vec::new();

    for i in 1..=8 {
        let action = Action::new(
            "messaging",
            "tenant-c",
            "premium-sms",
            "send_sms",
            serde_json::json!({"to": format!("+1555000{i:04}"), "body": format!("Message #{i}")}),
        );

        let outcome = gateway.dispatch(action, None).await?;

        match &outcome {
            ActionOutcome::Executed(_) => {
                executed_premium += 1;
                info!("  [dispatch #{i}] Executed via premium-sms (within quota)");
            }
            ActionOutcome::QuotaExceeded {
                limit,
                used,
                overage_behavior,
                ..
            } => {
                degraded += 1;
                info!(
                    "  [dispatch #{i}] QuotaExceeded — limit={limit}, used={used}, behavior={overage_behavior}"
                );
                // Parse the fallback provider from the overage_behavior string.
                if let Some(provider) = overage_behavior.strip_prefix("degrade:") {
                    fallback_providers.push(provider.to_string());
                    info!("  [dispatch #{i}] -> Caller should re-route to: {provider}");
                }
            }
            other => {
                info!("  [dispatch #{i}] Unexpected: {}", outcome_label(other));
            }
        }
    }

    let snap = gateway.metrics().snapshot();

    info!("");
    info!("  ┌─────────────────────────────────────────┐");
    info!("  │  Results                                 │");
    info!("  ├─────────────────────────────────────────┤");
    info!("  │  Executed (premium): {executed_premium:>3}                   │");
    info!("  │  Degraded:           {degraded:>3}                   │");
    info!(
        "  │  quota_degraded:     {:>3}                   │",
        snap.quota_degraded
    );
    info!(
        "  │  Fallback provider:  {:>18}   │",
        fallback_providers.first().map_or("-", String::as_str)
    );
    info!("  └─────────────────────────────────────────┘");

    assert_eq!(
        executed_premium, 5,
        "expected 5 actions on premium provider"
    );
    assert_eq!(degraded, 3, "expected 3 actions degraded");
    assert_eq!(snap.quota_degraded, 3);
    assert!(
        fallback_providers.iter().all(|p| p == "log-fallback"),
        "all degraded actions should reference log-fallback"
    );

    info!("");

    // =========================================================================
    // Summary Table
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║              TENANT USAGE QUOTAS DEMO COMPLETE               ║");
    info!("╠══════════════════════════════════════════════════════════════╣");
    info!("║                                                              ║");
    info!("║  ┌──────────┬───────┬──────────┬─────────┬────────────────┐  ║");
    info!("║  │ Tenant   │ Limit │ Behavior │ Sent    │ Result         │  ║");
    info!("║  ├──────────┼───────┼──────────┼─────────┼────────────────┤  ║");
    info!("║  │ tenant-a │ 10/hr │ Block    │ 15      │ 10 ok, 5 deny  │  ║");
    info!("║  │ tenant-b │100/dy │ Warn     │ 105     │ 105 ok, 5 warn │  ║");
    info!("║  │ tenant-c │  5/hr │ Degrade  │ 8       │ 5 ok, 3 degrad │  ║");
    info!("║  └──────────┴───────┴──────────┴─────────┴────────────────┘  ║");
    info!("║                                                              ║");
    info!("║  Key takeaways:                                              ║");
    info!("║                                                              ║");
    info!("║  1. Block — hard limit, excess actions rejected outright     ║");
    info!("║  2. Warn  — soft limit, all actions proceed, metrics track   ║");
    info!("║  3. Degrade — excess actions report fallback provider for    ║");
    info!("║     caller-side re-routing to a cheaper alternative          ║");
    info!("║                                                              ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}

// =============================================================================
// Helper functions
// =============================================================================

/// Return a short label for an outcome variant (for display purposes).
fn outcome_label(outcome: &ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Executed(_) => "Executed",
        ActionOutcome::Deduplicated => "Deduplicated",
        ActionOutcome::Suppressed { .. } => "Suppressed",
        ActionOutcome::Rerouted { .. } => "Rerouted",
        ActionOutcome::Throttled { .. } => "Throttled",
        ActionOutcome::Failed(_) => "Failed",
        ActionOutcome::Grouped { .. } => "Grouped",
        ActionOutcome::StateChanged { .. } => "StateChanged",
        ActionOutcome::PendingApproval { .. } => "PendingApproval",
        ActionOutcome::ChainStarted { .. } => "ChainStarted",
        ActionOutcome::DryRun { .. } => "DryRun",
        ActionOutcome::CircuitOpen { .. } => "CircuitOpen",
        ActionOutcome::Scheduled { .. } => "Scheduled",
        ActionOutcome::RecurringCreated { .. } => "RecurringCreated",
        ActionOutcome::QuotaExceeded { .. } => "QuotaExceeded",
    }
}
