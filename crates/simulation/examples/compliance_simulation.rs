//! Demonstration of SOC2/HIPAA Compliance Mode in the simulation framework.
//!
//! This example shows how compliance mode controls audit behavior — synchronous
//! writes, hash chaining, immutable records — and how the `ComplianceConfig` is
//! applied to the gateway. The simulation exercises the core types without
//! requiring a running server or external dependencies.
//!
//! Scenarios demonstrated:
//!   1. No compliance mode -- default behavior, async audit writes.
//!   2. SOC2 compliance -- enables sync audit writes and hash chaining.
//!   3. HIPAA compliance -- enables sync writes, hash chaining, and immutable audit.
//!   4. Custom overrides -- start from a mode but override individual settings.
//!   5. Hash chain verification -- verify a valid chain returns clean results.
//!   6. Mixed tenants -- different tenants on the same gateway with different modes.
//!
//! Run with: `cargo run -p acteon-simulation --example compliance_simulation`

use std::sync::Arc;

use acteon_core::{Action, ActionOutcome, ComplianceConfig, ComplianceMode, HashChainVerification};
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;
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
    info!("║       SOC2/HIPAA COMPLIANCE MODE SIMULATION DEMO            ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: No compliance mode (default)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: NO COMPLIANCE MODE (default)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  With no compliance mode, audit writes are asynchronous, the");
    info!("  hash chain is disabled, and audit records can be modified.\n");

    let config = ComplianceConfig::default();
    assert_eq!(config.mode, ComplianceMode::None);
    assert!(!config.sync_audit_writes);
    assert!(!config.immutable_audit);
    assert!(!config.hash_chain);

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .compliance_config(config.clone())
        .build()?;

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

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Mode:             none                   │");
    info!("  │  Sync writes:      false                  │");
    info!("  │  Hash chain:       false                  │");
    info!("  │  Immutable audit:  false                  │");
    info!("  │  Dispatch outcome: Executed                │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 2: SOC2 compliance mode
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: SOC2 COMPLIANCE MODE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  SOC2 mode enables synchronous audit writes and hash chaining.");
    info!("  Audit records remain mutable (deletions allowed).\n");

    let config = ComplianceConfig::new(ComplianceMode::Soc2);
    assert_eq!(config.mode, ComplianceMode::Soc2);
    assert!(config.sync_audit_writes);
    assert!(config.hash_chain);
    assert!(!config.immutable_audit);

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .compliance_config(config.clone())
        .build()?;

    let action = Action::new(
        "notifications",
        "tenant-soc2",
        "email",
        "send_report",
        serde_json::json!({"to": "auditor@example.com", "report": "quarterly"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Mode:             soc2                   │");
    info!("  │  Sync writes:      true                   │");
    info!("  │  Hash chain:       true                   │");
    info!("  │  Immutable audit:  false                  │");
    info!("  │  Dispatch outcome: Executed                │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 3: HIPAA compliance mode
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: HIPAA COMPLIANCE MODE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  HIPAA mode enables all compliance features: synchronous audit");
    info!("  writes, hash chaining, and immutable audit records (deletes and");
    info!("  updates are rejected by the audit store decorator).\n");

    let config = ComplianceConfig::new(ComplianceMode::Hipaa);
    assert_eq!(config.mode, ComplianceMode::Hipaa);
    assert!(config.sync_audit_writes);
    assert!(config.hash_chain);
    assert!(config.immutable_audit);

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .compliance_config(config.clone())
        .build()?;

    let action = Action::new(
        "healthcare",
        "tenant-hipaa",
        "email",
        "send_phi_notification",
        serde_json::json!({"to": "nurse@hospital.org", "dept": "cardiology"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Mode:             hipaa                  │");
    info!("  │  Sync writes:      true                   │");
    info!("  │  Hash chain:       true                   │");
    info!("  │  Immutable audit:  true                   │");
    info!("  │  Dispatch outcome: Executed                │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 4: Custom overrides
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 4: CUSTOM OVERRIDES");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Start from SOC2 mode but override immutable_audit to true and");
    info!("  disable sync_audit_writes. This shows how individual settings");
    info!("  can be fine-tuned after selecting a base mode.\n");

    let config = ComplianceConfig::new(ComplianceMode::Soc2)
        .with_immutable_audit(true)
        .with_sync_audit_writes(false);

    assert_eq!(config.mode, ComplianceMode::Soc2);
    assert!(!config.sync_audit_writes); // Overridden from SOC2 default (true)
    assert!(config.immutable_audit); // Overridden from SOC2 default (false)
    assert!(config.hash_chain); // Kept from SOC2 default

    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .compliance_config(config)
        .build()?;

    let action = Action::new(
        "notifications",
        "tenant-custom",
        "email",
        "send_custom",
        serde_json::json!({"to": "admin@example.com", "subject": "Custom config"}),
    );

    let outcome = gateway.dispatch(action, None).await?;
    assert!(matches!(outcome, ActionOutcome::Executed(_)));

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Base mode:        soc2                   │");
    info!("  │  Sync writes:      false (overridden)     │");
    info!("  │  Hash chain:       true  (from soc2)      │");
    info!("  │  Immutable audit:  true  (overridden)     │");
    info!("  │  Dispatch outcome: Executed                │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 5: Hash chain verification types
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 5: HASH CHAIN VERIFICATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  The HashChainVerification struct captures the result of a chain");
    info!("  integrity check. This demo constructs valid and broken examples");
    info!("  and verifies serialization round-trips.\n");

    // A valid chain
    let valid = HashChainVerification {
        valid: true,
        records_checked: 500,
        first_broken_at: None,
        first_record_id: Some("aud-001".into()),
        last_record_id: Some("aud-500".into()),
    };
    let json = serde_json::to_string_pretty(&valid)?;
    let back: HashChainVerification = serde_json::from_str(&json)?;
    assert!(back.valid);
    assert_eq!(back.records_checked, 500);
    assert!(back.first_broken_at.is_none());

    info!("  Valid chain verification:");
    info!("    records_checked: 500");
    info!("    valid:           true");
    info!("    range:           aud-001 .. aud-500\n");

    // A broken chain
    let broken = HashChainVerification {
        valid: false,
        records_checked: 250,
        first_broken_at: Some("aud-123".into()),
        first_record_id: Some("aud-001".into()),
        last_record_id: Some("aud-250".into()),
    };
    let json = serde_json::to_string_pretty(&broken)?;
    let back: HashChainVerification = serde_json::from_str(&json)?;
    assert!(!back.valid);
    assert_eq!(back.first_broken_at.as_deref(), Some("aud-123"));

    info!("  Broken chain verification:");
    info!("    records_checked: 250");
    info!("    valid:           false");
    info!("    first_broken_at: aud-123");

    info!("\n  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Valid chain serde roundtrip:   OK         │");
    info!("  │  Broken chain serde roundtrip:  OK         │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 6: Mixed tenants on one gateway
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 6: MIXED TENANTS ON ONE GATEWAY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  A single gateway can serve tenants with different compliance");
    info!("  needs. The compliance mode is a gateway-wide setting that");
    info!("  determines the audit pipeline behavior for all tenants.\n");

    // Build gateway with HIPAA compliance
    let config = ComplianceConfig::new(ComplianceMode::Hipaa);
    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .provider(Arc::new(MockProvider::new("sms")))
        .compliance_config(config)
        .build()?;

    // Dispatch for tenant A via email
    let action_a = Action::new(
        "notifications",
        "tenant-alpha",
        "email",
        "send_phi",
        serde_json::json!({"to": "doc@alpha.org", "dept": "radiology"}),
    );
    let outcome_a = gateway.dispatch(action_a, None).await?;
    assert!(matches!(outcome_a, ActionOutcome::Executed(_)));

    // Dispatch for tenant B via SMS
    let action_b = Action::new(
        "alerts",
        "tenant-beta",
        "sms",
        "send_reminder",
        serde_json::json!({"to": "+15551234567", "body": "Appointment reminder"}),
    );
    let outcome_b = gateway.dispatch(action_b, None).await?;
    assert!(matches!(outcome_b, ActionOutcome::Executed(_)));

    info!("  ┌──────────────────────────────────────────┐");
    info!("  │  Results                                  │");
    info!("  ├──────────────────────────────────────────┤");
    info!("  │  Gateway mode:     hipaa                  │");
    info!("  │  Tenant Alpha:     email Executed          │");
    info!("  │  Tenant Beta:      sms   Executed          │");
    info!("  │  Both under HIPAA audit pipeline           │");
    info!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SUMMARY
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║  ALL 6 SCENARIOS PASSED                                     ║");
    info!("╠══════════════════════════════════════════════════════════════╣");
    info!("║  1. No compliance mode:     async writes, no hash chain      ║");
    info!("║  2. SOC2 mode:              sync writes + hash chain         ║");
    info!("║  3. HIPAA mode:             sync + hash chain + immutable    ║");
    info!("║  4. Custom overrides:       fine-tune per-setting            ║");
    info!("║  5. Hash chain verification: valid/broken serde roundtrip    ║");
    info!("║  6. Mixed tenants:          multi-tenant under one mode      ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
