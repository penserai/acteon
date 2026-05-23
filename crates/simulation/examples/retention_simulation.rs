//! Demonstration of Data Retention Policies in the simulation framework.
//!
//! This example shows how per-tenant retention policies control the audit TTL
//! resolution and expose retention-related metrics on the gateway. Retention
//! policies are checked during audit recording to determine how long records
//! are kept, and a background reaper (not exercised here) uses the state/event
//! TTLs to clean up old data.
//!
//! Scenarios demonstrated:
//!   1. Default TTL -- no retention policy; the gateway-wide `audit_ttl_seconds`
//!      is used as-is.
//!   2. Per-tenant TTL override -- a retention policy with `audit_ttl_seconds`
//!      takes precedence over the gateway default.
//!   3. Compliance hold -- when `compliance_hold` is true, the effective TTL
//!      is `None` (audit records never expire), regardless of any TTL setting.
//!   4. Disabled policy -- a disabled retention policy is ignored; the gateway
//!      default applies.
//!   5. Multiple tenants -- different tenants can have different retention
//!      policies on the same gateway instance.
//!   6. Metrics -- retention-related metrics are accessible via the metrics
//!      snapshot.
//!
//! Run with: `cargo run -p acteon-simulation --example retention_simulation`

use std::collections::HashMap;
use std::sync::Arc;

use acteon_core::{Action, ActionOutcome, RetentionPolicy};
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
    info!("║          DATA RETENTION POLICIES SIMULATION DEMO             ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    let now = Utc::now();

    // =========================================================================
    // SCENARIO 1: Default TTL (no retention policy)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: DEFAULT TTL (no retention policy)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  When no retention policy is set for a tenant, the gateway-wide");
    info!("  audit_ttl_seconds is used. Here we set it to 86400 (24 hours).\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400)
        .build()?;

    info!("  Gateway built with audit_ttl_seconds = 86400 (24h)");
    info!("  No retention policy registered.\n");

    // Dispatch an action to verify the gateway works.
    let action = Action::new(
        "notifications",
        "tenant-default",
        "email",
        "send_alert",
        serde_json::json!({"to": "admin@example.com", "subject": "Test"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(
        matches!(outcome, ActionOutcome::Executed(_)),
        "expected Executed outcome"
    );

    // Check that no retention policies are loaded.
    let policies = gateway.retention_policies();
    assert!(
        policies.is_empty(),
        "expected no retention policies on gateway"
    );

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Retention policies:  0                   │");
    info!("  │  Dispatch outcome:    Executed             │");
    info!("  │  Effective audit TTL: gateway default (24h)│");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 2: Per-tenant audit TTL override
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: PER-TENANT AUDIT TTL OVERRIDE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant A has a retention policy with audit_ttl_seconds = 2592000");
    info!("  (30 days). This overrides the gateway default of 86400 (24h).\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400) // Gateway default: 24 hours
        .retention_policy(RetentionPolicy {
            id: "ret-tenant-a".into(),
            namespace: "notifications".into(),
            tenant: "tenant-a".into(),
            enabled: true,
            audit_ttl_seconds: Some(2_592_000), // 30 days
            state_ttl_seconds: Some(604_800),   // 7 days
            event_ttl_seconds: Some(259_200),   // 3 days
            compliance_hold: false,
            created_at: now,
            updated_at: now,
            description: Some("Tenant A: 30-day audit retention".into()),
            labels: Default::default(),
        })
        .build()?;

    info!("  Gateway built with:");
    info!("    - Global audit TTL:  86400s (24h)");
    info!("    - Tenant A audit TTL: 2592000s (30d)\n");

    let action = Action::new(
        "notifications",
        "tenant-a",
        "email",
        "send_alert",
        serde_json::json!({"to": "admin@tenant-a.com", "subject": "Monthly report"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    // Verify the policy is loaded.
    let policies = gateway.retention_policies();
    assert_eq!(policies.len(), 1, "expected 1 retention policy");

    let policy = policies
        .get("notifications:tenant-a")
        .expect("policy for tenant-a should exist");
    assert_eq!(policy.audit_ttl_seconds, Some(2_592_000));
    assert_eq!(policy.state_ttl_seconds, Some(604_800));
    assert_eq!(policy.event_ttl_seconds, Some(259_200));
    assert!(!policy.compliance_hold);
    assert!(policy.enabled);

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Retention policies:  1                   │");
    info!("  │  Tenant A audit TTL:  2592000s (30 days)  │");
    info!("  │  Tenant A state TTL:  604800s (7 days)    │");
    info!("  │  Tenant A event TTL:  259200s (3 days)    │");
    info!("  │  Compliance hold:     false                │");
    info!("  │  Dispatch outcome:    Executed             │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 3: Compliance hold (never-expire)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: COMPLIANCE HOLD (audit records never expire)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant B has compliance_hold = true. Even though audit_ttl_seconds");
    info!("  is set, the effective TTL is None (records never expire). This is");
    info!("  essential for GDPR, SOC2, and HIPAA compliance scenarios.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400) // Gateway default: 24 hours
        .retention_policy(RetentionPolicy {
            id: "ret-tenant-b".into(),
            namespace: "notifications".into(),
            tenant: "tenant-b".into(),
            enabled: true,
            audit_ttl_seconds: Some(86_400), // This is overridden by compliance_hold
            state_ttl_seconds: None,
            event_ttl_seconds: None,
            compliance_hold: true, // <-- Never expire audit records
            created_at: now,
            updated_at: now,
            description: Some("Tenant B: HIPAA compliance hold".into()),
            labels: {
                let mut m = HashMap::new();
                m.insert("compliance".into(), "hipaa".into());
                m.insert("tier".into(), "enterprise".into());
                m
            },
        })
        .build()?;

    info!("  Gateway built with:");
    info!("    - Global audit TTL:       86400s (24h)");
    info!("    - Tenant B audit TTL:     86400s (set but overridden)");
    info!("    - Tenant B compliance_hold: true\n");

    let action = Action::new(
        "notifications",
        "tenant-b",
        "email",
        "send_alert",
        serde_json::json!({"to": "compliance@tenant-b.com", "subject": "Audit event"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    let policies = gateway.retention_policies();
    let policy = policies
        .get("notifications:tenant-b")
        .expect("policy for tenant-b should exist");
    assert!(policy.compliance_hold, "compliance_hold should be true");
    assert_eq!(policy.labels.get("compliance"), Some(&"hipaa".to_string()));
    assert_eq!(policy.labels.get("tier"), Some(&"enterprise".to_string()));

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Compliance hold:     true                │");
    info!("  │  Effective audit TTL: None (never expire)  │");
    info!("  │  Labels:              compliance=hipaa     │");
    info!("  │  Dispatch outcome:    Executed             │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 4: Disabled retention policy
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 4: DISABLED RETENTION POLICY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Tenant C has a retention policy with enabled = false. The policy");
    info!("  is registered but ignored; the gateway default TTL applies.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400) // Gateway default
        .retention_policy(RetentionPolicy {
            id: "ret-tenant-c".into(),
            namespace: "notifications".into(),
            tenant: "tenant-c".into(),
            enabled: false, // <-- Disabled
            audit_ttl_seconds: Some(999_999),
            state_ttl_seconds: Some(999_999),
            event_ttl_seconds: Some(999_999),
            compliance_hold: false,
            created_at: now,
            updated_at: now,
            description: Some("Tenant C: disabled policy".into()),
            labels: Default::default(),
        })
        .build()?;

    info!("  Gateway built with:");
    info!("    - Global audit TTL:  86400s (24h)");
    info!("    - Tenant C enabled:  false\n");

    let action = Action::new(
        "notifications",
        "tenant-c",
        "email",
        "send_alert",
        serde_json::json!({"to": "admin@tenant-c.com"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    let policies = gateway.retention_policies();
    let policy = policies
        .get("notifications:tenant-c")
        .expect("policy for tenant-c should exist");
    assert!(!policy.enabled, "policy should be disabled");

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Policy enabled:       false               │");
    info!("  │  Effective audit TTL:  gateway default (24h)│");
    info!("  │  Dispatch outcome:     Executed             │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 5: Multiple tenants with different policies
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 5: MULTIPLE TENANTS WITH DIFFERENT POLICIES");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Three tenants on a single gateway, each with different retention");
    info!("  requirements:\n");
    info!("    - free-tier:    no retention policy (gateway default: 24h)");
    info!("    - pro-tier:     90-day audit, 30-day state, 14-day events");
    info!("    - enterprise:   compliance hold (never expire)\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400) // 24 hours default
        .retention_policy(RetentionPolicy {
            id: "ret-pro".into(),
            namespace: "notifications".into(),
            tenant: "pro-tier".into(),
            enabled: true,
            audit_ttl_seconds: Some(7_776_000), // 90 days
            state_ttl_seconds: Some(2_592_000), // 30 days
            event_ttl_seconds: Some(1_209_600), // 14 days
            compliance_hold: false,
            created_at: now,
            updated_at: now,
            description: Some("Pro tier: 90-day audit retention".into()),
            labels: {
                let mut m = HashMap::new();
                m.insert("tier".into(), "pro".into());
                m
            },
        })
        .retention_policy(RetentionPolicy {
            id: "ret-enterprise".into(),
            namespace: "notifications".into(),
            tenant: "enterprise".into(),
            enabled: true,
            audit_ttl_seconds: None,
            state_ttl_seconds: None,
            event_ttl_seconds: None,
            compliance_hold: true,
            created_at: now,
            updated_at: now,
            description: Some("Enterprise: compliance hold".into()),
            labels: {
                let mut m = HashMap::new();
                m.insert("tier".into(), "enterprise".into());
                m
            },
        })
        .build()?;

    info!("  Gateway built with 2 retention policies.\n");

    // Dispatch one action per tenant.
    let tenants = ["free-tier", "pro-tier", "enterprise"];
    let mut results: Vec<(&str, &str)> = Vec::new();

    for tenant in tenants {
        let action = Action::new(
            "notifications",
            tenant,
            "email",
            "send_report",
            serde_json::json!({"to": format!("admin@{tenant}.com")}),
        );

        let outcome = gateway.dispatch(action, None).await?;
        let status = match &outcome {
            ActionOutcome::Executed(_) => "Executed",
            _ => "Unexpected",
        };
        results.push((tenant, status));
        info!("  [{tenant}] dispatched -> {status}");
    }

    let policies = gateway.retention_policies();
    assert_eq!(
        policies.len(),
        2,
        "expected 2 retention policies (free-tier has none)"
    );
    assert!(
        policies.contains_key("notifications:pro-tier"),
        "pro-tier policy should exist"
    );
    assert!(
        policies.contains_key("notifications:enterprise"),
        "enterprise policy should exist"
    );
    assert!(
        !policies.contains_key("notifications:free-tier"),
        "free-tier should have no policy"
    );

    let pro = policies.get("notifications:pro-tier").unwrap();
    assert_eq!(pro.audit_ttl_seconds, Some(7_776_000));
    assert!(!pro.compliance_hold);

    let ent = policies.get("notifications:enterprise").unwrap();
    assert!(ent.compliance_hold);

    info!("");
    info!("  ┌───────────────┬──────────────────────────┬────────────┐");
    info!("  │ Tenant        │ Effective Audit TTL       │ Hold       │");
    info!("  ├───────────────┼──────────────────────────┼────────────┤");
    info!("  │ free-tier     │ 86400s (gateway default)  │ false      │");
    info!("  │ pro-tier      │ 7776000s (90 days)        │ false      │");
    info!("  │ enterprise    │ None (never expire)       │ true       │");
    info!("  └───────────────┴──────────────────────────┴────────────┘\n");

    // =========================================================================
    // SCENARIO 6: Runtime policy management and metrics
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 6: RUNTIME POLICY MANAGEMENT AND METRICS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Retention policies can be added, updated, and removed at runtime");
    info!("  without restarting the gateway. Retention metrics track reaper");
    info!("  activity.\n");

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .audit_ttl_seconds(86_400)
        .build()?;

    // Initially no policies.
    assert!(gateway.retention_policies().is_empty());
    info!("  1. Initial state: 0 retention policies");

    // Add a policy at runtime.
    gateway.set_retention_policy(RetentionPolicy {
        id: "ret-runtime".into(),
        namespace: "analytics".into(),
        tenant: "dynamic-tenant".into(),
        enabled: true,
        audit_ttl_seconds: Some(172_800), // 2 days
        state_ttl_seconds: Some(86_400),  // 1 day
        event_ttl_seconds: None,
        compliance_hold: false,
        created_at: now,
        updated_at: now,
        description: Some("Dynamically added policy".into()),
        labels: Default::default(),
    });

    assert_eq!(gateway.retention_policies().len(), 1);
    info!("  2. Added policy at runtime: analytics:dynamic-tenant (2-day audit)");

    // Update the policy.
    gateway.set_retention_policy(RetentionPolicy {
        id: "ret-runtime".into(),
        namespace: "analytics".into(),
        tenant: "dynamic-tenant".into(),
        enabled: true,
        audit_ttl_seconds: Some(604_800), // Updated to 7 days
        state_ttl_seconds: Some(172_800), // Updated to 2 days
        event_ttl_seconds: Some(86_400),  // Added 1-day event TTL
        compliance_hold: false,
        created_at: now,
        updated_at: Utc::now(),
        description: Some("Updated policy".into()),
        labels: Default::default(),
    });

    let policies = gateway.retention_policies();
    let updated = policies.get("analytics:dynamic-tenant").unwrap();
    assert_eq!(updated.audit_ttl_seconds, Some(604_800));
    assert_eq!(updated.state_ttl_seconds, Some(172_800));
    assert_eq!(updated.event_ttl_seconds, Some(86_400));
    info!("  3. Updated policy: audit TTL 2d -> 7d, added event TTL");

    // Remove the policy.
    let removed = gateway.remove_retention_policy("analytics", "dynamic-tenant");
    assert!(removed.is_some(), "should have removed the policy");
    assert!(gateway.retention_policies().is_empty());
    info!("  4. Removed policy: 0 policies remaining");

    // Check retention metrics (all zero since no reaper ran).
    let snap = gateway.metrics().snapshot();
    info!("");
    info!("  ┌────────────────────────────────────────────┐");
    info!("  │  Retention Metrics                          │");
    info!("  ├────────────────────────────────────────────┤");
    info!(
        "  │  retention_deleted_state:      {:>3}          │",
        snap.retention_deleted_state
    );
    info!(
        "  │  retention_skipped_compliance: {:>3}          │",
        snap.retention_skipped_compliance
    );
    info!(
        "  │  retention_errors:             {:>3}          │",
        snap.retention_errors
    );
    info!("  └────────────────────────────────────────────┘");

    assert_eq!(snap.retention_deleted_state, 0);
    assert_eq!(snap.retention_skipped_compliance, 0);
    assert_eq!(snap.retention_errors, 0);

    info!("");

    // =========================================================================
    // Summary Table
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║          DATA RETENTION POLICIES DEMO COMPLETE               ║");
    info!("╠══════════════════════════════════════════════════════════════╣");
    info!("║                                                              ║");
    info!("║  ┌──────────────┬──────────┬────────────┬─────────────────┐  ║");
    info!("║  │ Scenario     │ TTL Src  │ Hold       │ Effective TTL   │  ║");
    info!("║  ├──────────────┼──────────┼────────────┼─────────────────┤  ║");
    info!("║  │ No policy    │ Gateway  │ false      │ 86400s (24h)    │  ║");
    info!("║  │ Per-tenant   │ Policy   │ false      │ 2592000s (30d)  │  ║");
    info!("║  │ Compliance   │ Policy   │ true       │ None (forever)  │  ║");
    info!("║  │ Disabled     │ Gateway  │ false      │ 86400s (24h)    │  ║");
    info!("║  │ Multi-tenant │ Mixed    │ mixed      │ Varies          │  ║");
    info!("║  │ Runtime mgmt │ Dynamic  │ false      │ Dynamic         │  ║");
    info!("║  └──────────────┴──────────┴────────────┴─────────────────┘  ║");
    info!("║                                                              ║");
    info!("║  Key takeaways:                                              ║");
    info!("║                                                              ║");
    info!("║  1. Three-level TTL resolution:                              ║");
    info!("║     compliance_hold > policy TTL > gateway default           ║");
    info!("║  2. Disabled policies are transparent (gateway default)      ║");
    info!("║  3. Policies can be managed at runtime without restart       ║");
    info!("║  4. Background reaper uses state/event TTLs for cleanup      ║");
    info!("║  5. Compliance hold preserves audit records indefinitely     ║");
    info!("║                                                              ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
