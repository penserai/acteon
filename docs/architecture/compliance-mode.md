# Compliance Mode Architecture

## Overview

Compliance mode configures the audit pipeline for regulatory requirements
(SOC2, HIPAA). It is implemented as a set of audit store decorators that wrap
the underlying `AuditStore` implementation, plus a dispatch pipeline flag that
controls synchronous vs. asynchronous audit writes.

The design follows the decorator pattern: each compliance feature is a separate
layer that wraps the inner store, so features compose independently and the
underlying backend remains unaware of compliance logic.

**Important**: Hash chaining (`hash_chain = true`) requires the `postgres` or
`dynamodb` audit backend for production deployments. See
[Backend Requirements](#10-backend-requirements) for details.

---

## 1. Data Model

### `ComplianceMode` (enum, lives in `acteon-core`)

```rust
pub enum ComplianceMode {
    None,   // No compliance constraints
    Soc2,   // Sync writes + hash chain
    Hipaa,  // Sync writes + hash chain + immutable audit
}
```

### `ComplianceConfig` (struct, lives in `acteon-core`)

```rust
pub struct ComplianceConfig {
    pub mode: ComplianceMode,
    pub sync_audit_writes: bool,
    pub immutable_audit: bool,
    pub hash_chain: bool,
}
```

Key design choices:

- **Mode + individual flags**: The mode selects sensible defaults, but each flag
  can be overridden independently via `with_*` builder methods or TOML overrides.
  This avoids a combinatorial explosion of modes while preserving a simple
  "pick a mode" experience.
- **Gateway-wide scope**: Compliance mode applies to all tenants on a gateway
  instance. Per-tenant compliance is not supported because regulatory posture is
  typically an infrastructure-level decision, not a tenant-level one.
- **Serde defaults**: All boolean fields use `#[serde(default)]`, so deserializing
  a partial JSON/TOML object produces `false` for unset flags rather than errors.

### `HashChainVerification` (struct, lives in `acteon-core`)

```rust
pub struct HashChainVerification {
    pub valid: bool,
    pub records_checked: u64,
    pub first_broken_at: Option<String>,
    pub first_record_id: Option<String>,
    pub last_record_id: Option<String>,
}
```

### `AuditRecord` Hash Chain Fields

Three optional fields are added to `AuditRecord` (in `acteon-audit`):

| Field | Type | Description |
|-------|------|-------------|
| `record_hash` | `Option<String>` | SHA-256 hex digest of canonicalized record content |
| `previous_hash` | `Option<String>` | Hash of the previous record in the chain |
| `sequence_number` | `Option<u64>` | Monotonic counter within `(namespace, tenant)` pair |

All three use `#[serde(default)]` for backward compatibility with existing records.

---

## 2. Decorator Composition

The audit store decorator stack is assembled at gateway build time:

```
                      +-----------------------+
                      | ComplianceAuditStore  |  (immutability enforcement)
                      |   inner: ───────────┐ |
                      +─────────────────────┘ |
                                              v
                      +-----------------------+
                      | HashChainAuditStore   |  (SHA-256 hash chaining)
                      |   inner: ───────────┐ |
                      +─────────────────────┘ |
                                              v
                      +-----------------------+
                      | Concrete AuditStore   |  (MemoryAuditStore,
                      | (backend)             |   PostgresAuditStore, etc.)
                      +-----------------------+
```

### Wrapping Order

The order matters: `ComplianceAuditStore` wraps `HashChainAuditStore`, which
wraps the concrete backend. This means:

1. A `record()` call first hits the compliance decorator, which only enforces
   immutability on delete/update operations (record writes pass through).
2. Then it hits the hash chain decorator, which computes the hash and links it
   to the previous record before forwarding to the backend.
3. The backend persists the enriched record.

For `query()` and `get_*()` calls, all decorators delegate directly to the
inner store without modification.

### Conditional Assembly

Not all decorators are always active:

| Configuration | Decorator Stack |
|---------------|----------------|
| `hash_chain = false, immutable_audit = false` | Backend only |
| `hash_chain = true, immutable_audit = false` | HashChain -> Backend |
| `hash_chain = false, immutable_audit = true` | Compliance -> Backend |
| `hash_chain = true, immutable_audit = true` | Compliance -> HashChain -> Backend |

The `GatewayBuilder` assembles the correct stack based on the `ComplianceConfig`.

---

## 3. Hash Chain Algorithm

### Record Hashing

The `HashChainAuditStore` maintains a per-chain state keyed by
`(namespace, tenant)`. For each incoming record:

```
1. Canonicalize the record content:
   - Extract the deterministic fields: action_id, namespace, tenant,
     provider, action_type, verdict, outcome, dispatched_at, completed_at
   - Serialize as a sorted JSON object (keys in alphabetical order)
   - This excludes volatile fields (id, metadata) from the hash to
     ensure deterministic verification

2. Compute the record hash:
   record_hash = SHA-256(canonical_json)

3. Link to previous record:
   previous_hash = last_hash_for(namespace, tenant)
                   or "genesis" if this is the first record

4. Assign sequence number:
   sequence_number = last_sequence_for(namespace, tenant) + 1
                     or 1 if this is the first record

5. Update chain state:
   set last_hash_for(namespace, tenant) = record_hash
   set last_sequence_for(namespace, tenant) = sequence_number

6. Set fields on the AuditRecord and forward to inner store
```

### Chain State Storage (Optimistic Concurrency)

The hash chain decorator does **not** maintain local state. Each write fetches
the current chain tip (latest `record_hash` and `sequence_number`) from the
database via a descending query limited to 1 row. This design is correct
across multiple gateway replicas.

A UNIQUE index on `(namespace, tenant, sequence_number)` in the database
enforces that no two records can share the same sequence number. If two
replicas race on the same chain, one write succeeds and the other receives a
unique constraint violation. The losing replica retries with jittered
exponential backoff (10ms base, doubling per attempt, up to 5 attempts),
re-fetching the tip on each retry.

### Verification Algorithm

The `POST /v1/audit/verify` endpoint implements:

```
1. Query all records for (namespace, tenant) ordered by sequence_number ASC
   - Optionally filtered by time range (from, to)

2. For each record in sequence:
   a. Re-canonicalize the record content using the same algorithm
   b. Recompute expected_hash = SHA-256(canonical_json)
   c. Verify record.record_hash == expected_hash
   d. Verify record.previous_hash == previous_record.record_hash
      (or "genesis" for the first record)
   e. Verify record.sequence_number == expected_sequence

3. If any check fails:
   - Set valid = false
   - Set first_broken_at = the failing record's ID
   - Stop verification (fail fast)

4. Return HashChainVerification result
```

### Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Hash algorithm | SHA-256 | Industry standard, sufficient for tamper detection |
| Canonicalization | Sorted JSON of deterministic fields | Reproducible across implementations |
| Chain scope | Per `(namespace, tenant)` | Matches the natural data partition boundary |
| Genesis marker | String `"genesis"` | Distinguishes first record from missing data |
| Fail-fast verification | Stop at first break | Faster for broken chains; full scan available via range queries |

---

## 4. Immutability Enforcement

### `ComplianceAuditStore` Behavior

The compliance decorator intercepts mutating operations:

| Operation | `immutable_audit = false` | `immutable_audit = true` |
|-----------|--------------------------|--------------------------|
| `record()` | Pass through | Pass through |
| `get_by_action_id()` | Pass through | Pass through |
| `get_by_id()` | Pass through | Pass through |
| `query()` | Pass through | Pass through |
| `cleanup_expired()` | Pass through | Reject (return error) |

When a mutating operation is rejected, the error includes a `COMPLIANCE_VIOLATION`
code to distinguish it from backend errors.

### Interaction with Data Retention

The immutability enforcement interacts with the retention reaper:

- When `immutable_audit = true`, the `cleanup_expired()` method on the
  compliance decorator returns an error, preventing the reaper from deleting
  audit records.
- When a retention policy has `compliance_hold = true`, the reaper skips the
  tenant entirely (checked before reaching the audit store).
- Both mechanisms can be active simultaneously for defense in depth.

---

## 5. Synchronous Audit Writes

### Dispatch Pipeline Integration

The `sync_audit_writes` flag controls the audit write path in the dispatch
pipeline:

```
Normal mode (sync_audit_writes = false):
  1. Build audit record
  2. tokio::spawn(audit_store.record(record))    ← fire and forget
  3. Return dispatch outcome immediately

Compliance mode (sync_audit_writes = true):
  1. Build audit record
  2. audit_store.record(record).await             ← inline await
  3. Return dispatch outcome after write confirmed
```

### Trade-offs

| Aspect | Async (default) | Sync (compliance) |
|--------|----------------|-------------------|
| Dispatch latency | Lower (audit write off critical path) | Higher (audit write on critical path) |
| Audit completeness | Best effort (crash before write = lost record) | Guaranteed (response implies persisted) |
| Throughput | Higher (write buffered) | Lower (serialized with dispatch) |
| Regulatory compliance | Insufficient for SOC2/HIPAA | Meets audit trail requirements |

---

## 6. Gateway Integration

### Builder

The `GatewayBuilder` accepts a `ComplianceConfig` via the `compliance_config()`
method:

```rust
let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .compliance_config(ComplianceConfig::new(ComplianceMode::Soc2))
    .build()?;
```

At build time, the builder:

1. Reads the `ComplianceConfig`
2. Wraps the audit store with the appropriate decorators
3. Stores the config on the `Gateway` struct for status reporting

### Dispatch Pipeline

```
1.  Acquire distributed lock
2.  check_quota(&action)
3.  Build eval context + evaluate rules
4.  Handle verdict (execute, suppress, reroute, ...)
5.  Build audit record
6.  Hash chain decorator: compute hash, link to previous
7.  Compliance decorator: enforce immutability
8.  Write audit record (sync when sync_audit_writes = true)
9.  Return outcome
```

Steps 6-7 are transparent to the gateway -- they happen inside the
`audit_store.record()` call through the decorator chain.

### Status Reporting

The `Gateway` exposes the current compliance configuration for the status API:

```rust
pub fn compliance_config(&self) -> &ComplianceConfig
```

---

## 7. Server Configuration

### TOML Configuration

```toml
[compliance]
mode = "soc2"              # "none", "soc2", or "hipaa"
# Optional overrides:
# sync_audit_writes = true
# immutable_audit = false
# hash_chain = true
```

The server config parser (`ComplianceConfigToml`) applies overrides on top of
the mode defaults: first `ComplianceConfig::new(mode)` is called to get mode
defaults, then any explicit overrides are applied.

### API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/compliance/status` | Returns current `ComplianceConfig` as JSON |
| `POST` | `/v1/audit/verify` | Verifies hash chain integrity for a namespace/tenant |

Both endpoints are read-only. Compliance mode cannot be changed at runtime --
it requires a configuration change and server restart.

---

## 8. Module / File Layout

### Core types

```
crates/core/src/compliance.rs     -- ComplianceMode, ComplianceConfig,
                                     HashChainVerification structs
crates/core/src/lib.rs            -- pub mod compliance; re-exports
```

### Audit decorators

```
crates/audit/audit/src/record.rs  -- AuditRecord hash chain fields
                                     (record_hash, previous_hash,
                                      sequence_number)
crates/audit/audit/src/store.rs   -- AuditStore trait (unchanged)
```

### Gateway

```
crates/gateway/src/gateway.rs     -- compliance_config field,
                                     sync audit write logic in dispatch
crates/gateway/src/builder.rs     -- compliance_config() builder method,
                                     decorator assembly at build time
```

### Server

```
crates/server/src/config.rs       -- ComplianceConfigToml, mode + overrides
crates/server/src/api/compliance.rs -- GET /v1/compliance/status handler
crates/server/src/api/audit.rs    -- POST /v1/audit/verify handler
```

---

## 9. Summary of Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Decorator pattern on `AuditStore` | Composable, backend-agnostic, single responsibility |
| Scope | Gateway-wide (not per-tenant) | Regulatory posture is infrastructure-level |
| Mode + overrides | Enum defaults + boolean overrides | Simple UX with escape hatches |
| Hash algorithm | SHA-256 | Standard, fast, sufficient for integrity checking |
| Chain scope | Per `(namespace, tenant)` | Matches data partition; avoids cross-tenant ordering |
| Sync writes | Flag on dispatch pipeline | Clear trade-off; easy to benchmark impact |
| Immutability | Decorator rejects deletes | Transparent to backend; no schema changes needed |
| Runtime changes | Not supported (restart required) | Compliance config changes should be deliberate |
| Verification | On-demand API endpoint | Avoids background overhead; callable on schedule |
| Canonicalization | Deterministic sorted JSON | Reproducible across implementations and languages |
| Backend requirement | Postgres or DynamoDB for hash chaining | Atomic uniqueness enforcement needed for CAS correctness |
| Concurrency model | Optimistic (CAS) with jittered backoff | No distributed coordination; scales horizontally |

---

## 10. Backend Requirements

### Why Postgres Is Required for Hash Chaining

Hash chain correctness depends on **exactly-once sequence number assignment**
across concurrent writers. This requires a database that can atomically reject
a duplicate `(namespace, tenant, sequence_number)` tuple at write time.

| Backend | Synchronous uniqueness enforcement | Hash chain support |
|---------|-----------------------------------|-------------------|
| `postgres` | Yes (`UNIQUE INDEX`) | Full support |
| `dynamodb` | Yes (`TransactWriteItems` + `attribute_not_exists`) | Full support |
| `memory` | Yes (in-process `HashMap`) | Testing/development only |
| `clickhouse` | No (mutations are async) | Not supported |
| `elasticsearch` | No (no UNIQUE constraint) | Not supported |

### Startup Validation

The server validates the backend at startup. If `hash_chain = true` and the
configured audit backend is not `postgres`, `dynamodb`, or `memory`, the server
exits immediately with an error message explaining the incompatibility. This
fail-fast approach prevents silent data integrity issues that would be
difficult to diagnose in production.

### Concurrency Model

**Postgres** uses UNIQUE constraint violations:

```
Replica A                    Postgres                    Replica B
    |                           |                           |
    |-- fetch tip (seq=41) ---->|                           |
    |                           |<-- fetch tip (seq=41) ----|
    |                           |                           |
    |-- INSERT seq=42 --------->|                           |
    |<-- OK                     |                           |
    |                           |<-- INSERT seq=42 ---------|
    |                           |--- UNIQUE violation ----->|
    |                           |                           |
    |                           |<-- fetch tip (seq=42) ----|
    |                           |<-- INSERT seq=43 ---------|
    |                           |--- OK ------------------->|
```

**DynamoDB** uses `TransactWriteItems` with a fence item:

```
Replica A                    DynamoDB                    Replica B
    |                           |                           |
    |-- query tip (seq=41) ---->|                           |
    |                           |<-- query tip (seq=41) ----|
    |                           |                           |
    |-- TransactWrite           |                           |
    |   (fence SEQ#42 +         |                           |
    |    audit record) -------->|                           |
    |<-- OK                     |                           |
    |                           |<-- TransactWrite ---------|
    |                           |    (fence SEQ#42 +        |
    |                           |     audit record)         |
    |                           |--- ConditionalCheckFailed>|
    |                           |                           |
    |                           |<-- query tip (seq=42) ----|
    |                           |<-- TransactWrite ---------|
    |                           |    (fence SEQ#43 + record)|
    |                           |--- OK ------------------->|
```

The jittered exponential backoff prevents thundering herd effects when many
replicas contend on the same chain simultaneously. The jitter is derived from
`SystemTime` nanoseconds to avoid deterministic collision patterns.
