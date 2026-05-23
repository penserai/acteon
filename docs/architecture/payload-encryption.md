# ADR: Payload Encryption at Rest

## Status

Accepted (updated with key rotation and extended encryption scope)

## Context

Action payloads flowing through Acteon can contain sensitive data: PII, API credentials, business-critical information. These payloads are persisted in state backends (for scheduling, chains, approvals, recurring actions, state machines, groups) and in audit backends. A database compromise, backup leak, or unauthorized DBA access could expose this data.

Acteon already has AES-256-GCM primitives in `acteon-crypto` for encrypting auth configuration secrets. This ADR extends that capability to action payloads at rest, including key rotation support and comprehensive state entry encryption.

## Decision

### Gateway-level encryption

We chose to encrypt at the **gateway level** rather than at the state store level or field level within JSON:

- **State store wrapper** was rejected because encryption must be selective (only payload-bearing keys, not counters/locks/indices). A generic wrapper would encrypt everything, including queryable metadata.
- **Field-level encryption** within JSON payloads was rejected as overly complex for v1. It would require schema awareness and make the encrypted payload structure harder to reason about.
- **Gateway-level** provides the right granularity: the gateway knows which state operations carry payload data and can selectively encrypt/decrypt at the boundaries.

### Separate key

`ACTEON_PAYLOAD_KEY` / `ACTEON_PAYLOAD_KEYS` is separate from `ACTEON_AUTH_KEY`:

- Different rotation lifecycle (auth keys rotate with credential changes; payload keys rotate with compliance cycles).
- Different blast radius (auth key compromise affects authentication; payload key compromise affects stored data confidentiality).
- Principle of least privilege: a service that only needs to authenticate doesn't need the payload key.

### Key rotation design

The `PayloadEncryptor` supports multiple named keys via `PayloadKeyEntry`:

```rust
pub struct PayloadKeyEntry {
    pub kid: String,     // Key identifier embedded in envelope
    pub key: MasterKey,  // AES-256 key material
}
```

- **Encryption** always uses the first key (`keys[0]`), embedding its `kid` in the envelope.
- **Decryption** extracts `kid` from envelope for direct key lookup. If the `kid` is not found or missing (legacy), falls back to trying all keys in order.
- **Envelope format**: `ENC[AES256-GCM,kid:<id>,data:<b64>,iv:<b64>,tag:<b64>]`. The `kid` field is optional for backward compatibility with pre-rotation envelopes.
- **Server config**: `ACTEON_PAYLOAD_KEYS="kid:hex,kid:hex,..."` (first key encrypts, all decrypt). Falls back to `ACTEON_PAYLOAD_KEY` for single-key backward compat.

### Wrapping order for audit

```
EncryptingAuditStore(RedactingAuditStore(InnerStore))
```

Redaction runs first on plaintext so that:
1. Redacted fields are removed before encryption (defense in depth -- even if encryption is broken, redacted data is gone).
2. The encrypted blob is smaller (redacted fields replaced with short placeholders).

### Backward compatibility

The `decrypt_str` / `decrypt_json` methods detect the `ENC[...]` envelope. If the input doesn't match, it's returned as-is. This means:
- Pre-encryption records remain readable after enabling encryption.
- Mixed encrypted/unencrypted data coexists during migration.
- Legacy envelopes (no `kid`) are decryptable via key fallback.
- No mandatory data migration step is required.

## Data Flow

### State store (scheduled action example)

```
handle_schedule()
  |-- serialize ScheduledAction to JSON
  |-- gateway.encrypt_state_value(json)  -->  ENC[AES256-GCM,kid:k1,...]
  +-- state.set(key, encrypted_value)

process_scheduled_actions() [background]
  |-- state.get(key)  -->  ENC[AES256-GCM,kid:k1,...]
  |-- processor.decrypt_state_value(raw)  -->  JSON
  +-- deserialize ScheduledAction
```

### Audit trail

```
gateway.dispatch()
  |-- build AuditRecord { action_payload: Some(json) }
  |-- RedactingAuditStore.record(record)
  |   +-- redact sensitive fields in action_payload
  |-- EncryptingAuditStore.record(record)
  |   +-- encrypt action_payload --> Value::String("ENC[...]")
  +-- InnerStore.record(record)

query()
  |-- InnerStore.query()  -->  records with ENC[...] payloads
  |-- EncryptingAuditStore.decrypt_record()
  |   +-- decrypt Value::String("ENC[...]") --> original JSON
  +-- return decrypted records
```

## Encrypted Key Kinds

| Data | Key Kind | Write Location | Read Location |
|------|----------|----------------|---------------|
| Scheduled actions | `ScheduledAction` | `handle_schedule` | `process_scheduled_actions` |
| Chain state | `Chain` | `handle_chain`, `persist_chain_state` | `advance_chain`, `get_chain_status`, `cancel_chain` |
| Approval records | `Approval` | `handle_request_approval` | `get_approval_record`, `execute_approval_inner`, `list_pending_approvals` |
| Recurring actions | `RecurringAction` | recurring API handlers | `process_recurring_actions`, recurring API handlers |
| Event state | `EventState` | `handle_state_machine` | `handle_state_machine`, `process_timeouts` |
| Active events | `ActiveEvents` | `handle_state_machine` | inhibition lookups |
| Event timeouts | `EventTimeout` | `handle_state_machine` | `process_timeouts` |
| Group metadata + events | `Group` | `add_to_group` | `recover_groups` |
| DLQ entries | (in-memory) | `EncryptingDeadLetterSink::push` | `EncryptingDeadLetterSink::drain` |

**Not encrypted**: `Dedup`, `Counter`, `Lock`, `RateLimit`, `PendingScheduled`, `PendingRecurring`, `PendingGroups`, `Quota`, `QuotaUsage` -- these contain no payload data.

### DLQ encryption

The `EncryptingDeadLetterSink` wraps any `DeadLetterSink` implementation:

- **`push()`**: Serializes `action.payload` to JSON, encrypts to `ENC[...]`, replaces the payload with `Value::String(encrypted)`, then delegates to the inner sink.
- **`drain()`**: For each entry, if `action.payload` is a `Value::String` matching `ENC[...]`, decrypts and parses back to the original JSON value.
- **`len()` / `is_empty()`**: Delegate directly, no transformation.

This ensures that even the in-memory DLQ holds only ciphertext, and any future persistent DLQ backend inherits encryption automatically.

### Group event persistence

Group state entries now include the full `events: Vec<GroupedEvent>` and `labels: HashMap<String, String>` alongside the existing metadata. Since the entire JSON blob is encrypted before storage, event payloads are protected at rest. On recovery (`recover_groups()`), events and labels are deserialized from the stored JSON — old entries without these fields gracefully default to empty collections.

## Threat Model

| Threat | Mitigation |
|--------|-----------|
| Database compromise | Payloads encrypted; attacker sees `ENC[...]` blobs |
| Backup exposure | Same as above; backups contain ciphertext |
| Unauthorized DBA access | Cannot read payload without encryption key |
| Key compromise | Rotate key: add new key as primary in `ACTEON_PAYLOAD_KEYS`, old data remains readable |
| Side-channel via metadata | Audit non-payload fields remain queryable (namespace, tenant, outcome, timestamps) -- this is by design for operational needs |

## Limitations

1. **No KMS integration** in v1. The key is provided as an environment variable. Future work could support AWS KMS, GCP KMS, or HashiCorp Vault for key wrapping.
2. **No payload field-level queries** when encrypted. Full-text search on payloads requires application-level decryption.
3. **HMAC fingerprints** (SHA-256 over payload fields) are not yet salted. This is a known PII risk deferred for a separate change.

## Consequences

- Existing deployments are unaffected (opt-in, `encryption.enabled = false` by default).
- New deployments can enable encryption with a single config flag and env var.
- Key rotation is supported without downtime via `ACTEON_PAYLOAD_KEYS`.
- Performance impact is negligible (AES-256-GCM is hardware-accelerated on modern CPUs).
- SDKs require no changes (encryption is server-side transparent).
