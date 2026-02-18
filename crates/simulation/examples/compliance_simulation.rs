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
        println!(
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
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║       SOC2/HIPAA COMPLIANCE MODE SIMULATION DEMO            ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: No compliance mode (default)
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: NO COMPLIANCE MODE (default)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  With no compliance mode, audit writes are asynchronous, the");
    println!("  hash chain is disabled, and audit records can be modified.\n");

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

    println!("  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Mode:             none                   │");
    println!("  │  Sync writes:      false                  │");
    println!("  │  Hash chain:       false                  │");
    println!("  │  Immutable audit:  false                  │");
    println!("  │  Dispatch outcome: Executed                │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 2: SOC2 compliance mode
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: SOC2 COMPLIANCE MODE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  SOC2 mode enables synchronous audit writes and hash chaining.");
    println!("  Audit records remain mutable (deletions allowed).\n");

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

    println!("  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Mode:             soc2                   │");
    println!("  │  Sync writes:      true                   │");
    println!("  │  Hash chain:       true                   │");
    println!("  │  Immutable audit:  false                  │");
    println!("  │  Dispatch outcome: Executed                │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 3: HIPAA compliance mode
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: HIPAA COMPLIANCE MODE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  HIPAA mode enables all compliance features: synchronous audit");
    println!("  writes, hash chaining, and immutable audit records (deletes and");
    println!("  updates are rejected by the audit store decorator).\n");

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

    println!("  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Mode:             hipaa                  │");
    println!("  │  Sync writes:      true                   │");
    println!("  │  Hash chain:       true                   │");
    println!("  │  Immutable audit:  true                   │");
    println!("  │  Dispatch outcome: Executed                │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 4: Custom overrides
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: CUSTOM OVERRIDES");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Start from SOC2 mode but override immutable_audit to true and");
    println!("  disable sync_audit_writes. This shows how individual settings");
    println!("  can be fine-tuned after selecting a base mode.\n");

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

    println!("  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Base mode:        soc2                   │");
    println!("  │  Sync writes:      false (overridden)     │");
    println!("  │  Hash chain:       true  (from soc2)      │");
    println!("  │  Immutable audit:  true  (overridden)     │");
    println!("  │  Dispatch outcome: Executed                │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 5: Hash chain verification types
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: HASH CHAIN VERIFICATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  The HashChainVerification struct captures the result of a chain");
    println!("  integrity check. This demo constructs valid and broken examples");
    println!("  and verifies serialization round-trips.\n");

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

    println!("  Valid chain verification:");
    println!("    records_checked: 500");
    println!("    valid:           true");
    println!("    range:           aud-001 .. aud-500\n");

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

    println!("  Broken chain verification:");
    println!("    records_checked: 250");
    println!("    valid:           false");
    println!("    first_broken_at: aud-123");

    println!("\n  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Valid chain serde roundtrip:   OK         │");
    println!("  │  Broken chain serde roundtrip:  OK         │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SCENARIO 6: Mixed tenants on one gateway
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 6: MIXED TENANTS ON ONE GATEWAY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A single gateway can serve tenants with different compliance");
    println!("  needs. The compliance mode is a gateway-wide setting that");
    println!("  determines the audit pipeline behavior for all tenants.\n");

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

    println!("  ┌──────────────────────────────────────────┐");
    println!("  │  Results                                  │");
    println!("  ├──────────────────────────────────────────┤");
    println!("  │  Gateway mode:     hipaa                  │");
    println!("  │  Tenant Alpha:     email Executed          │");
    println!("  │  Tenant Beta:      sms   Executed          │");
    println!("  │  Both under HIPAA audit pipeline           │");
    println!("  └──────────────────────────────────────────┘\n");

    // =========================================================================
    // SUMMARY
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  ALL 6 SCENARIOS PASSED                                     ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  1. No compliance mode:     async writes, no hash chain      ║");
    println!("║  2. SOC2 mode:              sync writes + hash chain         ║");
    println!("║  3. HIPAA mode:             sync + hash chain + immutable    ║");
    println!("║  4. Custom overrides:       fine-tune per-setting            ║");
    println!("║  5. Hash chain verification: valid/broken serde roundtrip    ║");
    println!("║  6. Mixed tenants:          multi-tenant under one mode      ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
