# Compliance Mode (SOC2 / HIPAA)

Compliance mode configures the audit pipeline for regulatory requirements. Selecting a mode pre-configures sensible defaults for synchronous writes, hash chaining, and record immutability:

- **SOC2** -- synchronous audit writes and SHA-256 hash chaining across records
- **HIPAA** -- all SOC2 features plus immutable audit records (deletes and updates rejected)
- **None** -- default behavior with no compliance constraints

## How It Works

When compliance mode is enabled, the gateway wraps the audit store with two decorator layers:

1. **`HashChainAuditStore`** -- computes a SHA-256 hash of each record's content and links it to the previous record's hash, creating a tamper-evident chain per `(namespace, tenant)` pair. Each record receives `record_hash`, `previous_hash`, and a monotonic `sequence_number`.

2. **`ComplianceAuditStore`** -- enforces immutability by rejecting delete and update operations on audit records. Only active when `immutable_audit = true`.

Additionally, when `sync_audit_writes = true`, the dispatch pipeline awaits the audit write inline rather than spawning it as a background task. This guarantees the audit record is persisted before the dispatch response is returned.

```
Dispatch Pipeline with Compliance Mode:

  1. Acquire distributed lock
  2. Check quotas
  3. Evaluate rules
  4. Execute action
  5. Build audit record
  6. Hash chain decorator:  compute hash, link to previous
  7. Compliance decorator:  enforce immutability
  8. Write audit record     (sync when sync_audit_writes = true)
  9. Return outcome
```

## Configuration

### Via the Gateway Builder (Rust)

```rust
use acteon_core::{ComplianceConfig, ComplianceMode};
use acteon_gateway::GatewayBuilder;

// SOC2: sync writes + hash chain
let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .compliance_config(ComplianceConfig::new(ComplianceMode::Soc2))
    .build()?;

// HIPAA: sync writes + hash chain + immutable
let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .compliance_config(ComplianceConfig::new(ComplianceMode::Hipaa))
    .build()?;

// Custom: start from SOC2 but add immutability
let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .compliance_config(
        ComplianceConfig::new(ComplianceMode::Soc2)
            .with_immutable_audit(true)
    )
    .build()?;
```

### Via TOML Configuration

```toml
[compliance]
mode = "soc2"              # "none", "soc2", or "hipaa"
# Optional overrides (uncomment to override mode defaults):
# sync_audit_writes = true
# immutable_audit = false
# hash_chain = true
```

### Via the REST API

Check the current compliance status:

```bash
curl http://localhost:8080/v1/compliance/status
```

Response:

```json
{
  "mode": "soc2",
  "sync_audit_writes": true,
  "immutable_audit": false,
  "hash_chain": true
}
```

## Mode Defaults

| Setting | None | SOC2 | HIPAA |
|---------|------|------|-------|
| `sync_audit_writes` | false | **true** | **true** |
| `hash_chain` | false | **true** | **true** |
| `immutable_audit` | false | false | **true** |

Each setting can be individually overridden after selecting a mode using the `with_*` builder methods or the TOML override fields.

## Hash Chain

When `hash_chain = true`, each audit record receives three additional fields:

| Field | Type | Description |
|-------|------|-------------|
| `record_hash` | `string` | SHA-256 hex digest of the canonicalized record content |
| `previous_hash` | `string` | Hash of the previous record in the chain |
| `sequence_number` | `integer` | Monotonic counter within the `(namespace, tenant)` pair |

The first record in a chain has `previous_hash = "genesis"` and `sequence_number = 1`.

### Verification

Verify the integrity of the hash chain for a namespace/tenant pair:

```bash
curl -X POST http://localhost:8080/v1/audit/verify \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "acme"
  }'
```

Optional parameters:

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Required. Namespace to verify. |
| `tenant` | string | Required. Tenant to verify. |
| `from` | string | Optional. Start of the time range (ISO 8601). |
| `to` | string | Optional. End of the time range (ISO 8601). |

Response:

```json
{
  "valid": true,
  "records_checked": 1523,
  "first_broken_at": null,
  "first_record_id": "aud-001",
  "last_record_id": "aud-1523"
}
```

If the chain is broken:

```json
{
  "valid": false,
  "records_checked": 500,
  "first_broken_at": "aud-237",
  "first_record_id": "aud-001",
  "last_record_id": "aud-500"
}
```

## Immutable Audit

When `immutable_audit = true`, the `ComplianceAuditStore` decorator rejects:

- **Delete operations** on individual audit records
- **Bulk delete** / purge operations

Attempts to delete or modify audit records will return an error:

```json
{
  "code": "COMPLIANCE_VIOLATION",
  "message": "Audit records are immutable in the current compliance mode"
}
```

This interacts with [data retention policies](data-retention.md): when both `immutable_audit` and `compliance_hold` are active, audit records are fully protected from both manual deletion and automated cleanup.

## Synchronous Audit Writes

When `sync_audit_writes = true`, the dispatch pipeline blocks until the audit record is confirmed written to the backend. This guarantees:

- Every executed action has a corresponding audit record
- No audit record is lost due to process crash between dispatch and async write
- Regulatory requirements for complete audit trails are satisfied

The trade-off is higher dispatch latency (the audit write is on the critical path).

## API Reference

### `GET /v1/compliance/status`

Returns the current compliance configuration.

**Response (200):**

```json
{
  "mode": "hipaa",
  "sync_audit_writes": true,
  "immutable_audit": true,
  "hash_chain": true
}
```

### `POST /v1/audit/verify`

Verify the integrity of the hash chain for a namespace/tenant pair.

**Request body:**

```json
{
  "namespace": "notifications",
  "tenant": "acme",
  "from": "2026-01-01T00:00:00Z",
  "to": "2026-02-17T00:00:00Z"
}
```

**Response (200):**

```json
{
  "valid": true,
  "records_checked": 1523,
  "first_broken_at": null,
  "first_record_id": "aud-001",
  "last_record_id": "aud-1523"
}
```

## Usage Examples

### Enable SOC2 compliance

```bash
# In acteon.toml
[compliance]
mode = "soc2"
```

### Check compliance status

```bash
curl http://localhost:8080/v1/compliance/status
```

### Verify audit chain integrity

```bash
curl -X POST http://localhost:8080/v1/audit/verify \
  -H "Content-Type: application/json" \
  -d '{"namespace": "notifications", "tenant": "acme"}'
```

### Verify a time range

```bash
curl -X POST http://localhost:8080/v1/audit/verify \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "acme",
    "from": "2026-02-01T00:00:00Z",
    "to": "2026-02-17T00:00:00Z"
  }'
```

## Interaction with Other Features

- **[Data Retention](data-retention.md)**: Compliance hold on a retention policy prevents the reaper from deleting audit records. Combined with `immutable_audit`, records are fully protected.
- **[Payload Encryption](payload-encryption.md)**: Hash chaining operates on the encrypted payload. The hash covers the ciphertext, not the plaintext.
- **[Audit Trail](audit-trail.md)**: Compliance mode adds three fields (`record_hash`, `previous_hash`, `sequence_number`) to each audit record. These are visible in the audit API response and the Admin UI.

## Best Practices

- **Choose the right mode**: Use SOC2 for financial/operational audit trails. Use HIPAA for healthcare or any scenario requiring immutable records.
- **Start with SOC2**: If unsure, SOC2 provides strong audit guarantees without the operational constraints of full immutability.
- **Monitor dispatch latency**: Sync audit writes add latency. Use the provider health dashboard to track p95/p99 impact.
- **Verify periodically**: Run hash chain verification on a schedule (e.g., daily) to detect any data corruption early.
- **Combine with retention**: Use retention policies with `compliance_hold = true` for tenants under regulatory requirements.

## Limitations

- **Gateway-wide setting**: Compliance mode applies to all tenants on the gateway. Per-tenant compliance modes are not currently supported.
- **Write-time only**: Enabling hash chaining does not retroactively hash existing records. Only new records participate in the chain.
- **Performance impact**: Synchronous audit writes increase dispatch latency. Hash computation adds a small overhead per record.
- **No key rotation**: The hash chain does not support key rotation. If the hashing algorithm needs to change, a new chain must be started.
