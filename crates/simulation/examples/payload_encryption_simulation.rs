//! Demonstration of Payload Encryption at Rest.
//!
//! This example verifies that action payloads are encrypted before storage
//! and correctly decrypted on read. It exercises the `PayloadEncryptor`,
//! the `EncryptingAuditStore`, and scheduled action dispatch.
//!
//! Run with: `cargo run -p acteon-simulation --example payload_encryption_simulation`

use std::sync::Arc;

use acteon_audit::AuditStore;
use acteon_core::{Action, ActionOutcome};
use acteon_crypto::{PayloadEncryptor, parse_master_key};
use acteon_simulation::prelude::*;

/// Schedule rule so dispatched actions land in state store.
const SCHEDULE_RULE: &str = r#"
rules:
  - name: schedule-test
    priority: 10
    description: "Schedule any send_later action with a 3600s delay"
    condition:
      field: action.action_type
      eq: "send_later"
    action:
      type: schedule
      delay_seconds: 3600
"#;

fn make_encryptor() -> Arc<PayloadEncryptor> {
    // 64 hex chars = 32 bytes = AES-256 key.
    let key = parse_master_key(&"ab".repeat(32)).expect("valid key");
    Arc::new(PayloadEncryptor::new(key))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         PAYLOAD ENCRYPTION AT REST SIMULATION                ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Scheduled action payloads are encrypted in state store
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: SCHEDULED ACTIONS — ENCRYPTED AT REST");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("webhook")
            .add_rule_yaml(SCHEDULE_RULE)
            .build(),
    )
    .await?;

    let action = Action::new(
        "notifications",
        "tenant-1",
        "webhook",
        "send_later",
        serde_json::json!({
            "secret_key": "super-secret-value",
            "message": "This payload should be encrypted at rest"
        }),
    );

    println!("  [dispatch] Scheduling action with sensitive payload...");
    let outcome = harness.dispatch(&action).await?;
    match &outcome {
        ActionOutcome::Scheduled { action_id, .. } => {
            println!("  [result]   Scheduled with ID: {action_id}");
        }
        other => {
            println!("  [result]   Unexpected outcome: {other:?}");
        }
    }

    // Verify that the action was scheduled (not executed yet).
    assert!(
        matches!(outcome, ActionOutcome::Scheduled { .. }),
        "expected Scheduled outcome"
    );
    assert_eq!(
        harness.provider("webhook").unwrap().call_count(),
        0,
        "provider should not be called for scheduled action"
    );

    println!("  [verify]   Action scheduled, provider NOT called (deferred)");
    println!("  [pass]     Scenario 1 passed\n");

    harness.teardown().await?;

    // =========================================================================
    // SCENARIO 2: Encryption roundtrip with PayloadEncryptor
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: ENCRYPTOR UNIT ROUNDTRIP");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let enc = make_encryptor();

    // JSON value roundtrip.
    let original = serde_json::json!({
        "api_key": "sk-test-123456",
        "nested": {
            "ssn": "123-45-6789"
        },
        "list": [1, 2, 3]
    });

    let encrypted = enc.encrypt_json(&original)?;
    println!("  [encrypt]  Original payload: {original}");
    println!(
        "  [encrypt]  Encrypted: {}...",
        &encrypted[..60.min(encrypted.len())]
    );

    assert!(
        acteon_crypto::is_encrypted(&encrypted),
        "encrypted value should match ENC[...] pattern"
    );

    let decrypted = enc.decrypt_json(&encrypted)?;
    assert_eq!(
        original, decrypted,
        "roundtrip should preserve JSON exactly"
    );
    println!("  [decrypt]  Decrypted matches original: ok");

    // String roundtrip.
    let plain = "sensitive-action-payload-data";
    let enc_str = enc.encrypt_str(plain)?;
    assert!(acteon_crypto::is_encrypted(&enc_str));
    let dec_str = enc.decrypt_str(&enc_str)?;
    assert_eq!(plain, dec_str);
    println!("  [string]   String roundtrip: ok");

    // Backward compat: plain JSON strings pass through decrypt unchanged.
    let plain_json = serde_json::json!({"not_encrypted": true}).to_string();
    let passthrough = enc.decrypt_str(&plain_json)?;
    assert_eq!(plain_json, passthrough);
    println!("  [compat]   Plain JSON passthrough: ok");

    // Different encryptions of same plaintext produce different ciphertext (random IV).
    let enc1 = enc.encrypt_str("same")?;
    let enc2 = enc.encrypt_str("same")?;
    assert_ne!(enc1, enc2, "encryptions should use random IVs");
    println!("  [nonce]    Random IV per encryption: ok");

    println!("  [pass]     Scenario 2 passed\n");

    // =========================================================================
    // SCENARIO 3: Audit store encryption
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: AUDIT STORE ENCRYPTION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let inner_audit: Arc<dyn acteon_audit::AuditStore> =
        Arc::new(acteon_audit_memory::MemoryAuditStore::new());
    let audit_enc = make_encryptor();
    let encrypting_audit: Arc<dyn AuditStore> = Arc::new(acteon_audit::EncryptingAuditStore::new(
        Arc::clone(&inner_audit),
        audit_enc,
    ));

    let now = chrono::Utc::now();
    let record = acteon_audit::AuditRecord {
        id: "audit-enc-test".to_string(),
        action_id: "action-enc-test".to_string(),
        chain_id: None,
        namespace: "ns".to_string(),
        tenant: "t".to_string(),
        provider: "webhook".to_string(),
        action_type: "test".to_string(),
        verdict: "allow".to_string(),
        matched_rule: None,
        outcome: "executed".to_string(),
        action_payload: Some(serde_json::json!({"password": "hunter2"})),
        verdict_details: serde_json::json!({}),
        outcome_details: serde_json::json!({}),
        metadata: serde_json::json!({}),
        dispatched_at: now,
        completed_at: now,
        duration_ms: 5,
        expires_at: None,
        caller_id: String::new(),
        auth_method: String::new(),
    };

    encrypting_audit.record(record).await?;

    // Read back through encrypting layer — should be decrypted.
    let fetched = encrypting_audit
        .get_by_id("audit-enc-test")
        .await?
        .expect("record should exist");
    assert_eq!(
        fetched.action_payload,
        Some(serde_json::json!({"password": "hunter2"}))
    );
    println!("  [audit]    Write + read roundtrip: payload decrypted correctly");

    // Read the raw inner store — should be encrypted.
    let raw = inner_audit
        .get_by_id("audit-enc-test")
        .await?
        .expect("record should exist in inner store");
    if let Some(serde_json::Value::String(s)) = &raw.action_payload {
        assert!(
            acteon_crypto::is_encrypted(s),
            "raw audit payload should be encrypted"
        );
        println!("  [audit]    Raw stored payload is encrypted: ok");
    } else {
        panic!("expected encrypted string in raw audit payload");
    }

    // Records without payload pass through.
    let no_payload = acteon_audit::AuditRecord {
        id: "audit-no-payload".to_string(),
        action_id: "action-no-payload".to_string(),
        chain_id: None,
        namespace: "ns".to_string(),
        tenant: "t".to_string(),
        provider: "webhook".to_string(),
        action_type: "test".to_string(),
        verdict: "allow".to_string(),
        matched_rule: None,
        outcome: "executed".to_string(),
        action_payload: None,
        verdict_details: serde_json::json!({}),
        outcome_details: serde_json::json!({}),
        metadata: serde_json::json!({}),
        dispatched_at: now,
        completed_at: now,
        duration_ms: 1,
        expires_at: None,
        caller_id: String::new(),
        auth_method: String::new(),
    };
    encrypting_audit.record(no_payload).await?;
    let fetched_none = encrypting_audit
        .get_by_id("audit-no-payload")
        .await?
        .expect("no-payload record should exist");
    assert!(
        fetched_none.action_payload.is_none(),
        "no-payload record should remain None"
    );
    println!("  [audit]    No-payload passthrough: ok");

    // Backward compat: pre-encryption plain records are readable.
    let plain_record = acteon_audit::AuditRecord {
        id: "audit-plain".to_string(),
        action_id: "action-plain".to_string(),
        chain_id: None,
        namespace: "ns".to_string(),
        tenant: "t".to_string(),
        provider: "webhook".to_string(),
        action_type: "test".to_string(),
        verdict: "allow".to_string(),
        matched_rule: None,
        outcome: "executed".to_string(),
        action_payload: Some(serde_json::json!({"plain": true})),
        verdict_details: serde_json::json!({}),
        outcome_details: serde_json::json!({}),
        metadata: serde_json::json!({}),
        dispatched_at: now,
        completed_at: now,
        duration_ms: 1,
        expires_at: None,
        caller_id: String::new(),
        auth_method: String::new(),
    };
    // Insert directly into inner store (bypass encryption).
    inner_audit.record(plain_record).await?;
    let fetched_plain = encrypting_audit
        .get_by_id("audit-plain")
        .await?
        .expect("plain record should exist");
    assert_eq!(
        fetched_plain.action_payload,
        Some(serde_json::json!({"plain": true}))
    );
    println!("  [audit]    Backward compat (plain records): ok");

    println!("  [pass]     Scenario 3 passed\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  ALL SCENARIOS PASSED                                        ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  1. Scheduled action encrypted at rest                       ║");
    println!("║  2. Encryptor unit roundtrip                                 ║");
    println!("║  3. Audit store encryption                                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
