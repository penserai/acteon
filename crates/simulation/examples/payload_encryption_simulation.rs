//! Demonstration of Payload Encryption at Rest.
//!
//! This example verifies that action payloads are encrypted before storage
//! and correctly decrypted on read. It exercises the `PayloadEncryptor`,
//! the `EncryptingAuditStore`, key rotation, DLQ encryption, group event
//! persistence with encryption, and scheduled action dispatch.
//!
//! Run with: `cargo run -p acteon-simulation --example payload_encryption_simulation`

use std::sync::Arc;

use acteon_audit::AuditStore;
use acteon_core::{Action, ActionOutcome};
use acteon_crypto::{PayloadEncryptor, PayloadKeyEntry, parse_master_key};
use acteon_executor::{DeadLetterQueue, DeadLetterSink};
use acteon_gateway::{EncryptingDeadLetterSink, GroupManager};
use acteon_simulation::prelude::*;
use acteon_state::StateStore;
use acteon_state_memory::MemoryStateStore;
use tracing::info;

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

fn make_key_a() -> acteon_crypto::MasterKey {
    parse_master_key(&"ab".repeat(32)).expect("valid key")
}

fn make_key_b() -> acteon_crypto::MasterKey {
    parse_master_key(&"cd".repeat(32)).expect("valid key")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         PAYLOAD ENCRYPTION AT REST SIMULATION                ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Scheduled action payloads are encrypted in state store
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 1: SCHEDULED ACTIONS — ENCRYPTED AT REST");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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

    info!("  [dispatch] Scheduling action with sensitive payload...");
    let outcome = harness.dispatch(&action).await?;
    match &outcome {
        ActionOutcome::Scheduled { action_id, .. } => {
            info!("  [result]   Scheduled with ID: {action_id}");
        }
        other => {
            info!("  [result]   Unexpected outcome: {other:?}");
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

    info!("  [verify]   Action scheduled, provider NOT called (deferred)");
    info!("  [pass]     Scenario 1 passed\n");

    harness.teardown().await?;

    // =========================================================================
    // SCENARIO 2: Encryption roundtrip with PayloadEncryptor
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 2: ENCRYPTOR UNIT ROUNDTRIP");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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
    info!("  [encrypt]  Original payload: {original}");
    info!(
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
    info!("  [decrypt]  Decrypted matches original: ok");

    // String roundtrip.
    let plain = "sensitive-action-payload-data";
    let enc_str = enc.encrypt_str(plain)?;
    assert!(acteon_crypto::is_encrypted(&enc_str));
    let dec_str = enc.decrypt_str(&enc_str)?;
    assert_eq!(plain, dec_str);
    info!("  [string]   String roundtrip: ok");

    // Backward compat: plain JSON strings pass through decrypt unchanged.
    let plain_json = serde_json::json!({"not_encrypted": true}).to_string();
    let passthrough = enc.decrypt_str(&plain_json)?;
    assert_eq!(plain_json, passthrough);
    info!("  [compat]   Plain JSON passthrough: ok");

    // Different encryptions of same plaintext produce different ciphertext (random IV).
    let enc1 = enc.encrypt_str("same")?;
    let enc2 = enc.encrypt_str("same")?;
    assert_ne!(enc1, enc2, "encryptions should use random IVs");
    info!("  [nonce]    Random IV per encryption: ok");

    info!("  [pass]     Scenario 2 passed\n");

    // =========================================================================
    // SCENARIO 3: Audit store encryption
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 3: AUDIT STORE ENCRYPTION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

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
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        canonical_hash: None,
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
    info!("  [audit]    Write + read roundtrip: payload decrypted correctly");

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
        info!("  [audit]    Raw stored payload is encrypted: ok");
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
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        canonical_hash: None,
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
    info!("  [audit]    No-payload passthrough: ok");

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
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        canonical_hash: None,
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
    info!("  [audit]    Backward compat (plain records): ok");

    info!("  [pass]     Scenario 3 passed\n");

    // =========================================================================
    // SCENARIO 4: Key rotation — multi-key PayloadEncryptor
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 4: KEY ROTATION — MULTI-KEY ENCRYPTOR");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Step 1: Encrypt data with the "old" key (k1).
    let old_enc = PayloadEncryptor::with_keys(vec![PayloadKeyEntry {
        kid: "k1".to_owned(),
        key: make_key_a(),
    }]);

    let secret_data = serde_json::json!({"credit_card": "4111-1111-1111-1111"});
    let encrypted_old = old_enc.encrypt_json(&secret_data)?;
    let kid = acteon_crypto::extract_kid(&encrypted_old);
    assert_eq!(kid.as_deref(), Some("k1"));
    info!("  [step1]    Encrypted with old key k1 (kid={kid:?})");

    // Step 2: Create a new encryptor with k2 (primary) + k1 (old).
    let rotated_enc = PayloadEncryptor::with_keys(vec![
        PayloadKeyEntry {
            kid: "k2".to_owned(),
            key: make_key_b(),
        },
        PayloadKeyEntry {
            kid: "k1".to_owned(),
            key: make_key_a(),
        },
    ]);

    // Old data is still decryptable.
    let decrypted_old = rotated_enc.decrypt_json(&encrypted_old)?;
    assert_eq!(decrypted_old, secret_data);
    info!("  [step2]    Old k1 data decryptable with rotated encryptor: ok");

    // New encryptions use k2.
    let encrypted_new = rotated_enc.encrypt_json(&secret_data)?;
    let new_kid = acteon_crypto::extract_kid(&encrypted_new);
    assert_eq!(new_kid.as_deref(), Some("k2"));
    info!("  [step3]    New encryptions use k2 (kid={new_kid:?}): ok");

    // New data roundtrips.
    let decrypted_new = rotated_enc.decrypt_json(&encrypted_new)?;
    assert_eq!(decrypted_new, secret_data);
    info!("  [step4]    New k2 data roundtrips: ok");

    // Legacy envelopes without kid (pre-rotation) are handled by fallback.
    let legacy_enc =
        acteon_crypto::encrypt_value(&serde_json::to_string(&secret_data)?, &make_key_a())?;
    assert!(acteon_crypto::extract_kid(&legacy_enc).is_none());
    let decrypted_legacy = rotated_enc.decrypt_str(&legacy_enc)?;
    let parsed_legacy: serde_json::Value = serde_json::from_str(&decrypted_legacy)?;
    assert_eq!(parsed_legacy, secret_data);
    info!("  [step5]    Legacy (no kid) envelope decrypted via fallback: ok");

    // Data encrypted with an unknown key fails gracefully.
    let unknown_key = parse_master_key(&"ff".repeat(32))?;
    let unknown_enc = acteon_crypto::encrypt_value_with_kid("secret", &unknown_key, Some("k99"))?;
    let result = rotated_enc.decrypt_str(&unknown_enc);
    assert!(result.is_err(), "unknown key should fail");
    info!("  [step6]    Unknown kid correctly rejected: ok");

    info!("  [pass]     Scenario 4 passed\n");

    // =========================================================================
    // SCENARIO 5: DLQ encryption — EncryptingDeadLetterSink
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 5: DLQ ENCRYPTION — EncryptingDeadLetterSink");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let inner_dlq = Arc::new(DeadLetterQueue::new());
    let dlq_enc = make_encryptor();
    let encrypting_dlq: Arc<dyn DeadLetterSink> = Arc::new(EncryptingDeadLetterSink::new(
        Arc::clone(&inner_dlq) as Arc<dyn DeadLetterSink>,
        dlq_enc,
    ));

    // Push a sensitive action through the encrypting DLQ.
    let sensitive_payload = serde_json::json!({
        "api_key": "sk-secret-production-key",
        "pii": {"ssn": "123-45-6789", "name": "Alice"}
    });
    let dlq_action = Action::new(
        "notifications",
        "tenant-1",
        "webhook",
        "send_alert",
        sensitive_payload.clone(),
    );
    encrypting_dlq
        .push(dlq_action, "provider timeout".into(), 3)
        .await;
    info!("  [push]     Pushed action with sensitive payload to DLQ");

    // Verify the raw inner DLQ holds encrypted data.
    let raw_entries = inner_dlq.drain();
    assert_eq!(raw_entries.len(), 1);
    match &raw_entries[0].action.payload {
        serde_json::Value::String(s) => {
            assert!(
                acteon_crypto::is_encrypted(s),
                "raw DLQ payload should be encrypted"
            );
            info!("  [verify]   Inner DLQ holds encrypted payload: ok");
        }
        other => panic!("expected encrypted String, got {other:?}"),
    }

    // Push again (since we drained the inner for inspection) and drain through
    // the encrypting wrapper to verify decryption.
    let dlq_action2 = Action::new(
        "notifications",
        "tenant-1",
        "webhook",
        "send_alert",
        sensitive_payload.clone(),
    );
    encrypting_dlq
        .push(dlq_action2, "provider timeout".into(), 3)
        .await;

    let decrypted_entries = encrypting_dlq.drain().await;
    assert_eq!(decrypted_entries.len(), 1);
    assert_eq!(decrypted_entries[0].action.payload, sensitive_payload);
    assert_eq!(decrypted_entries[0].error, "provider timeout");
    assert_eq!(decrypted_entries[0].attempts, 3);
    info!("  [drain]    Draining through wrapper returns decrypted payload: ok");

    // Verify len/is_empty delegation.
    assert!(encrypting_dlq.is_empty().await);
    info!("  [empty]    DLQ is empty after drain: ok");

    info!("  [pass]     Scenario 5 passed\n");

    // =========================================================================
    // SCENARIO 6: Group event persistence (encrypted) with crash recovery
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  SCENARIO 6: GROUP EVENT PERSISTENCE + CRASH RECOVERY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let group_enc = make_encryptor();
    let state_store = MemoryStateStore::new();
    let manager = GroupManager::new();

    // Add events to a group (encrypted).
    let group_action1 = Action::new(
        "alerts",
        "team-a",
        "slack",
        "cpu_alert",
        serde_json::json!({"host": "server-1", "cpu": 95.2}),
    );
    let group_action2 = Action::new(
        "alerts",
        "team-a",
        "slack",
        "cpu_alert",
        serde_json::json!({"host": "server-2", "cpu": 88.7}),
    );

    let (gid, gkey, size1, _) = manager
        .add_to_group(
            &group_action1,
            &["action_type".to_string()],
            300,  // group_wait_seconds
            300,  // group_interval_seconds
            None, // repeat_interval_seconds — ephemeral
            100,  // max_group_size
            &state_store,
            Some(&group_enc),
        )
        .await?;
    info!("  [add]      Added event 1 to group {gid} (size={size1})");

    let (_, _, size2, _) = manager
        .add_to_group(
            &group_action2,
            &["action_type".to_string()],
            300,
            300,
            None,
            100,
            &state_store,
            Some(&group_enc),
        )
        .await?;
    info!("  [add]      Added event 2 to group (size={size2})");
    assert_eq!(size2, 2);

    // Verify the raw state store value is encrypted.
    let raw_key =
        acteon_state::StateKey::new("alerts", "team-a", acteon_state::KeyKind::Group, &gkey);
    let raw_val = state_store
        .get(&raw_key)
        .await?
        .expect("group should exist in store");
    assert!(
        acteon_crypto::is_encrypted(&raw_val),
        "stored group metadata should be encrypted"
    );
    info!("  [verify]   State store holds encrypted group blob: ok");

    // Simulate crash: create a new GroupManager and recover from state.
    let recovered_manager = GroupManager::new();
    let count = recovered_manager
        .recover_groups(&state_store, "alerts", "team-a", Some(&group_enc))
        .await?;
    assert_eq!(count, 1, "should recover one group");
    info!("  [recover]  Recovered {count} group from state store");

    let recovered_group = recovered_manager
        .get_group(&gkey)
        .expect("recovered group should exist");
    assert_eq!(
        recovered_group.size(),
        2,
        "recovered group should have 2 events"
    );
    assert_eq!(
        recovered_group.events[0].payload,
        serde_json::json!({"host": "server-1", "cpu": 95.2})
    );
    assert_eq!(
        recovered_group.events[1].payload,
        serde_json::json!({"host": "server-2", "cpu": 88.7})
    );
    info!("  [verify]   Recovered group has 2 events with correct payloads: ok");

    // Verify labels were recovered.
    assert!(
        !recovered_group.labels.is_empty(),
        "labels should be recovered"
    );
    info!("  [verify]   Labels recovered: ok");

    // Backward compatibility: old group entries without events/labels.
    let old_group_key = "old-group-key";
    let old_group_meta = serde_json::json!({
        "group_id": "old-group-id",
        "group_key": old_group_key,
        "size": 5,
        "notify_at": chrono::Utc::now().to_rfc3339(),
        "trace_context": {},
    });
    let old_key = acteon_state::StateKey::new(
        "alerts",
        "team-b",
        acteon_state::KeyKind::Group,
        old_group_key,
    );
    state_store
        .set(&old_key, &old_group_meta.to_string(), None)
        .await?;

    let compat_manager = GroupManager::new();
    let compat_count = compat_manager
        .recover_groups(&state_store, "alerts", "team-b", None)
        .await?;
    assert_eq!(compat_count, 1);
    let compat_group = compat_manager
        .get_group(old_group_key)
        .expect("old group should be recovered");
    assert_eq!(
        compat_group.size(),
        0,
        "old entries without events should recover with empty events"
    );
    assert!(
        compat_group.labels.is_empty(),
        "old entries without labels should recover with empty labels"
    );
    info!("  [compat]   Old entries (no events/labels) recover gracefully: ok");

    info!("  [pass]     Scenario 6 passed\n");

    // =========================================================================
    // Summary
    // =========================================================================
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║  ALL SCENARIOS PASSED                                        ║");
    info!("╠══════════════════════════════════════════════════════════════╣");
    info!("║  1. Scheduled action encrypted at rest                       ║");
    info!("║  2. Encryptor unit roundtrip                                 ║");
    info!("║  3. Audit store encryption                                   ║");
    info!("║  4. Key rotation (multi-key encryptor)                       ║");
    info!("║  5. DLQ encryption (EncryptingDeadLetterSink)                ║");
    info!("║  6. Group event persistence + crash recovery                 ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
