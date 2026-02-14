# ADR: Payload Encryption at Rest

## Status

Accepted

## Context

Action payloads flowing through Acteon can contain sensitive data: PII, API credentials, business-critical information. These payloads are persisted in state backends (for scheduling, chains, approvals, recurring actions) and in audit backends. A database compromise, backup leak, or unauthorized DBA access could expose this data.

Acteon already has AES-256-GCM primitives in `acteon-crypto` for encrypting auth configuration secrets. This ADR extends that capability to action payloads at rest.

## Decision

### Gateway-level encryption

We chose to encrypt at the **gateway level** rather than at the state store level or field level within JSON:

- **State store wrapper** was rejected because encryption must be selective (only payload-bearing keys, not counters/locks/indices). A generic wrapper would encrypt everything, including queryable metadata.
- **Field-level encryption** within JSON payloads was rejected as overly complex for v1. It would require schema awareness and make the encrypted payload structure harder to reason about.
- **Gateway-level** provides the right granularity: the gateway knows which state operations carry payload data and can selectively encrypt/decrypt at the boundaries.

### Separate key

`ACTEON_PAYLOAD_KEY` is separate from `ACTEON_AUTH_KEY`:

- Different rotation lifecycle (auth keys rotate with credential changes; payload keys rotate with compliance cycles).
- Different blast radius (auth key compromise affects authentication; payload key compromise affects stored data confidentiality).
- Principle of least privilege: a service that only needs to authenticate doesn't need the payload key.

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
- No mandatory data migration step is required.

## Data Flow

### State store (scheduled action example)

```
handle_schedule()
  ├─ serialize ScheduledAction to JSON
  ├─ gateway.encrypt_state_value(json)  →  ENC[AES256-GCM,...]
  └─ state.set(key, encrypted_value)

process_scheduled_actions() [background]
  ├─ state.get(key)  →  ENC[AES256-GCM,...]
  ├─ processor.decrypt_state_value(raw)  →  JSON
  └─ deserialize ScheduledAction
```

### Audit trail

```
gateway.dispatch()
  ├─ build AuditRecord { action_payload: Some(json) }
  ├─ RedactingAuditStore.record(record)
  │   └─ redact sensitive fields in action_payload
  ├─ EncryptingAuditStore.record(record)
  │   └─ encrypt action_payload → Value::String("ENC[...]")
  └─ InnerStore.record(record)

query()
  ├─ InnerStore.query()  →  records with ENC[...] payloads
  ├─ EncryptingAuditStore.decrypt_record()
  │   └─ decrypt Value::String("ENC[...]") → original JSON
  └─ return decrypted records
```

## Encrypted Key Kinds

| Data | Key Kind | Write Location | Read Location |
|------|----------|----------------|---------------|
| Scheduled actions | `ScheduledAction` | `handle_schedule` | `process_scheduled_actions` |
| Chain state | `Chain` | `handle_chain`, `persist_chain_state` | `advance_chain`, `get_chain_status`, `cancel_chain` |
| Approval records | `Approval` | `handle_request_approval` | `get_approval_record`, `execute_approval_inner`, `list_pending_approvals` |
| Recurring actions | `RecurringAction` | recurring API handlers | `process_recurring_actions`, recurring API handlers |

**Not encrypted**: `Dedup`, `Counter`, `Lock`, `RateLimit`, `PendingScheduled`, `PendingRecurring`, `Quota`, `QuotaUsage` -- these contain no payload data.

## Threat Model

| Threat | Mitigation |
|--------|-----------|
| Database compromise | Payloads encrypted; attacker sees `ENC[...]` blobs |
| Backup exposure | Same as above; backups contain ciphertext |
| Unauthorized DBA access | Cannot read payload without `ACTEON_PAYLOAD_KEY` |
| Key compromise | All stored payloads are exposed; rotate key + re-encrypt |
| Side-channel via metadata | Audit non-payload fields remain queryable (namespace, tenant, outcome, timestamps) -- this is by design for operational needs |

## Limitations

1. **No key rotation mechanism** in v1. Changing the key requires re-encrypting all stored data (future work: envelope encryption with key versioning).
2. **No KMS integration** in v1. The key is provided as an environment variable. Future work could support AWS KMS, GCP KMS, or HashiCorp Vault for key wrapping.
3. **No payload field-level queries** when encrypted. Full-text search on payloads requires application-level decryption.

## Consequences

- Existing deployments are unaffected (opt-in, `encryption.enabled = false` by default).
- New deployments can enable encryption with a single config flag and env var.
- Performance impact is negligible (AES-256-GCM is hardware-accelerated on modern CPUs).
- SDKs require no changes (encryption is server-side transparent).
