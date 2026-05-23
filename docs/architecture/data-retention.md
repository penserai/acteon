# Data Retention Policies Architecture

## Overview

Data retention policies provide per-tenant control over the lifecycle of audit
records, completed chain state, and resolved event records. The feature operates
at two levels: audit TTL resolution during dispatch (write-time) and a background
reaper for state-store cleanup (post-hoc).

Retention policies are defined as `RetentionPolicy` structs and can be registered
at build time (via `GatewayBuilder::retention_policy()`) or managed at runtime via
the REST API.

---

## 1. Data Model

### `RetentionPolicy` (core struct, lives in `acteon-core`)

```rust
pub struct RetentionPolicy {
    pub id: String,
    pub namespace: String,
    pub tenant: String,
    pub enabled: bool,
    pub audit_ttl_seconds: Option<u64>,
    pub state_ttl_seconds: Option<u64>,
    pub event_ttl_seconds: Option<u64>,
    pub compliance_hold: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub description: Option<String>,
    pub labels: HashMap<String, String>,
}
```

Key design choices:

- **All TTLs are optional**: A policy can override only the audit TTL, only the
  state TTL, or any combination. Unset TTLs fall through to the gateway default
  or remain unbounded.
- **`compliance_hold` is a separate flag**: Rather than encoding "never expire" as
  a sentinel TTL value (e.g., `u64::MAX`), a boolean flag makes intent explicit
  and is easier to audit.
- **`enabled` for soft disable**: Policies can be disabled without deletion, which
  is useful for testing and gradual rollout.

---

## 2. State Storage Design

### Key Kinds

| `KeyKind` | `as_str()` | Purpose |
|-----------|-----------|---------|
| `Retention` | `"retention"` | Stores the `RetentionPolicy` JSON definition |

### Key Layout

| Key | Format | TTL |
|-----|--------|-----|
| **Policy** | `_system:_retention:retention:{id}` | None (permanent until deleted) |
| **Index** | `_system:_retention:retention:idx:{namespace}:{tenant}` | None (permanent until deleted) |

The index key provides a fast lookup from `namespace:tenant` to policy ID,
enabling the conflict check on creation (only one policy per namespace:tenant).

### Gateway In-Memory Cache

Policies are also stored in a `HashMap<String, RetentionPolicy>` on the `Gateway`
struct, protected by a `parking_lot::RwLock`. This provides O(1) lookup during
dispatch without hitting the state store on every action.

The in-memory cache is populated:

1. At build time via `GatewayBuilder::retention_policy()`
2. On server startup by scanning `KeyKind::Retention` from the state store
3. At runtime via `Gateway::set_retention_policy()` (called by the API handlers)

The background reaper also reloads policies from the state store on each cycle,
ensuring hot-reload across distributed instances.

---

## 3. Three-Level Audit TTL Resolution

The `effective_audit_ttl()` method on `Gateway` implements the resolution:

```rust
fn effective_audit_ttl(&self, namespace: &str, tenant: &str) -> Option<u64> {
    let key = format!("{namespace}:{tenant}");
    if let Some(policy) = self.retention_policies.read().get(&key) {
        if policy.enabled {
            if policy.compliance_hold {
                return None; // Never expires
            }
            if let Some(ttl) = policy.audit_ttl_seconds {
                return Some(ttl);
            }
        }
    }
    self.audit_ttl_seconds
}
```

Resolution order (most specific wins):

| Priority | Condition | Result |
|----------|-----------|--------|
| 1 | `compliance_hold = true` (enabled policy) | `None` (never expires) |
| 2 | `audit_ttl_seconds` set (enabled policy) | Per-tenant TTL |
| 3 | No policy, disabled policy, or no audit TTL in policy | Gateway-wide `audit_ttl_seconds` |

This method is called in three places within the dispatch pipeline:

1. After quota-exceeded early return (audit recording of quota outcome)
2. After normal dispatch completion (main audit recording)
3. During chain step audit recording

---

## 4. Gateway Integration

### Dispatch Pipeline

The retention TTL resolution is **not** a pipeline step in the same way as quotas
or rules. Instead, it is a computation that feeds into the audit recording phase:

```
1.  Acquire distributed lock
2b. check_quota(&action)
3.  Build eval context + evaluate rules
3b. LLM guardrail
3c. Dry-run early return
4.  Handle verdict (execute, suppress, reroute, ...)
5.  Emit audit record ← effective_audit_ttl() called here
```

The effective TTL is passed to `build_audit_record()`, which sets the `expires_at`
field on the `AuditRecord`. The audit store backend is responsible for honoring
this expiry (e.g., `PostgreSQL` backends use row-level TTL, in-memory backends
check on read).

### Runtime Policy Management

The `Gateway` exposes three methods for runtime management:

```rust
// Read all policies (snapshot)
pub fn retention_policies(&self) -> HashMap<String, RetentionPolicy>

// Add or replace a policy
pub fn set_retention_policy(&self, policy: RetentionPolicy)

// Remove a policy by namespace:tenant
pub fn remove_retention_policy(&self, namespace: &str, tenant: &str)
    -> Option<RetentionPolicy>
```

These are called by the API handlers in `crates/server/src/api/retention.rs`.
The API also persists policies to the state store for cross-instance visibility.

---

## 5. Background Reaper Design

### Architecture

The reaper runs as a branch in the `BackgroundProcessor::run()` select loop,
alongside group flushing, chain advancement, scheduled actions, and recurring
actions. It is controlled by two configuration knobs:

```rust
pub enable_retention_reaper: bool,        // default: false
pub retention_check_interval: Duration,   // default: 3600 seconds
```

### Reaper Algorithm

```
1. Scan state store for all keys of kind Retention
2. Deserialize each key into a RetentionPolicy
3. For each enabled policy where compliance_hold = false:
   a. If state_ttl_seconds is set:
      - Scan for completed/failed/cancelled chains for this namespace:tenant
      - Delete chains older than state_ttl_seconds
      - Increment retention_deleted_state metric per deletion
   b. If event_ttl_seconds is set:
      - Scan for resolved events for this namespace:tenant
      - Delete events older than event_ttl_seconds
      - Increment retention_deleted_state metric per deletion
4. For each policy where compliance_hold = true:
   - Skip entirely
   - Increment retention_skipped_compliance metric
5. Log summary: deleted count, skipped count, error count
```

### Hot-Reload

The reaper reloads policies from the state store on every cycle (step 1). This
means policies created or updated via the API on any instance are visible to the
reaper on the next cycle without a restart. This is the same pattern used by
`process_recurring_actions()`.

### Distributed Safety

The reaper does not acquire per-tenant locks because:

- Deletions are idempotent (deleting an already-deleted key is a no-op)
- The reaper only targets terminal records (completed chains, resolved events)
  that are no longer being mutated by the dispatch pipeline
- In a multi-instance deployment, multiple reapers may run concurrently; at worst,
  they scan the same keys and issue redundant deletes

---

## 6. Compliance Hold Semantics

The `compliance_hold` flag is a coarse-grained mechanism designed for regulatory
scenarios where data must be preserved indefinitely:

| Aspect | `compliance_hold = false` | `compliance_hold = true` |
|--------|--------------------------|--------------------------|
| Audit TTL | Per-tenant or gateway default | `None` (never expires) |
| State reaper | Active (deletes old chains) | Skipped |
| Event reaper | Active (deletes old events) | Skipped |
| Metric | -- | `retention_skipped_compliance` |

### Interaction with `audit_ttl_seconds`

When `compliance_hold = true`, the `audit_ttl_seconds` field on the same policy
is ignored. This is intentional: compliance hold is an absolute override that
prevents any TTL from being applied. To resume normal retention, set
`compliance_hold = false` first.

### Interaction with Payload Encryption

Compliance hold does not affect payload encryption. Encrypted payloads under
compliance hold remain encrypted at rest and are decrypted on read as usual.

---

## 7. Metrics and Observability

### `GatewayMetrics` Fields

```rust
pub retention_deleted_state: AtomicU64,      // State entries deleted by reaper
pub retention_skipped_compliance: AtomicU64,  // Entries skipped (compliance hold)
pub retention_errors: AtomicU64,              // Reaper errors
```

These counters are included in the `MetricsSnapshot` returned by
`Gateway::metrics().snapshot()` and exposed via the `GET /health` endpoint.

### Structured Logging

The `run_retention_reaper` method emits:

- `info!` for cycle completion (with deleted, skipped, errors counts)
- `error!` for individual reap failures (with namespace, tenant, error)

---

## 8. Module / File Layout

### Core types

```
crates/core/src/retention.rs     -- RetentionPolicy struct, serde, tests
crates/core/src/lib.rs           -- pub mod retention; re-export
```

### State keys

```
crates/state/state/src/key.rs    -- KeyKind::Retention
```

### Gateway

```
crates/gateway/src/gateway.rs    -- retention_policies field,
                                    effective_audit_ttl(),
                                    set_retention_policy(),
                                    remove_retention_policy(),
                                    retention_policies()
crates/gateway/src/builder.rs    -- retention_policy(), retention_policies()
                                    builder methods
crates/gateway/src/metrics.rs    -- retention_deleted_state,
                                    retention_skipped_compliance,
                                    retention_errors counters
crates/gateway/src/background.rs -- run_retention_reaper() method,
                                    enable_retention_reaper config,
                                    retention_check_interval config
```

### Server

```
crates/server/src/api/retention.rs -- CRUD API handlers
crates/server/src/config.rs        -- enable_retention_reaper,
                                      retention_check_interval_seconds
crates/server/src/main.rs          -- Startup policy loading from state store
```

---

## 9. Summary of Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| TTL resolution | Three-level (hold > policy > gateway) | Clear precedence; compliance hold is absolute |
| Compliance hold | Separate boolean flag | Explicit intent; avoids sentinel TTL values |
| Policy storage | State store + in-memory cache | O(1) dispatch lookup; cross-instance persistence |
| Reaper frequency | Configurable (default 1 hour) | Balance between promptness and load |
| Reaper locking | None (idempotent deletes) | Terminal records are immutable; no race risk |
| Policy scope | One per namespace:tenant | Matches quota model; simple mental model |
| Audit TTL | Write-time (not retroactive) | Changing a policy does not rewrite history |
| Hot-reload | Reaper reloads from state store each cycle | No restart needed for policy changes |
| Disabled policy | Transparent (falls through to default) | Safe for testing new policies |
