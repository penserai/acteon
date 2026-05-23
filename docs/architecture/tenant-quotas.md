# Tenant Usage Quotas Architecture

## Overview

Tenant usage quotas enforce per-tenant limits on the number of actions dispatched
within a configurable time window. The feature integrates into the gateway dispatch
pipeline after lock acquisition but before rule evaluation, so quotas are checked
on every dispatch regardless of rule configuration.

Quotas are defined as `QuotaPolicy` structs and can be registered at build time
(via `GatewayBuilder::quota_policy()`) or managed at runtime via the REST API.

---

## 1. Data Model

### `QuotaPolicy` (core struct, lives in `acteon-core`)

```rust
pub struct QuotaPolicy {
    pub id: String,
    pub namespace: String,
    pub tenant: String,
    pub max_actions: u64,
    pub window: QuotaWindow,
    pub overage_behavior: OverageBehavior,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub description: Option<String>,
    pub labels: HashMap<String, String>,
}
```

### `QuotaWindow`

```rust
pub enum QuotaWindow {
    Hourly,           // 3,600s
    Daily,            // 86,400s
    Weekly,           // 604,800s
    Monthly,          // 2,592,000s
    Custom { seconds: u64 },
}
```

### `OverageBehavior`

```rust
pub enum OverageBehavior {
    Block,
    Warn,
    Degrade { fallback_provider: String },
    Notify { target: String },
}
```

### `QuotaUsage` (read-only query result)

```rust
pub struct QuotaUsage {
    pub tenant: String,
    pub namespace: String,
    pub used: u64,
    pub limit: u64,
    pub remaining: u64,
    pub window: QuotaWindow,
    pub resets_at: DateTime<Utc>,
    pub overage_behavior: OverageBehavior,
}
```

### `ActionOutcome::QuotaExceeded`

```rust
QuotaExceeded {
    tenant: String,
    limit: u64,
    used: u64,
    overage_behavior: String,
}
```

---

## 2. State Storage Design

### Key Kinds

| `KeyKind` | `as_str()` | Purpose |
|-----------|-----------|---------|
| `Quota` | `"quota"` | Stores the `QuotaPolicy` JSON definition (for API-managed policies) |
| `QuotaUsage` | `"quota_usage"` | Stores the rolling usage counter for a window |

### Key Layout

| Key | Format | TTL |
|-----|--------|-----|
| **Policy** | `{ns}:{tenant}:quota:{id}` | None (permanent until deleted) |
| **Usage counter** | `{ns}:{tenant}:quota_usage:{ns}:{tenant}:{window_label}:{window_index}` | Window duration (auto-expire) |

### Counter Key Construction

The `quota_counter_key()` function in `acteon-core` builds a deterministic key
from the namespace, tenant, window type, and epoch-aligned window index:

```rust
pub fn quota_counter_key(
    namespace: &str,
    tenant: &str,
    window: &QuotaWindow,
    now: &DateTime<Utc>,
) -> String {
    let epoch = DateTime::UNIX_EPOCH;
    let elapsed = now.signed_duration_since(epoch);
    let window_secs = window.duration_seconds() as i64;
    let window_index = elapsed.num_seconds() / window_secs;
    format!("{namespace}:{tenant}:{}:{window_index}", window.label())
}
```

### Epoch-Aligned Windows

All windows are aligned to the Unix epoch. The window index is computed as
`floor(unix_seconds / window_seconds)`. This means:

- **Hourly**: windows start at :00:00 of each hour
- **Daily**: windows start at 00:00:00 UTC each day
- **Weekly**: windows start every 604,800 seconds from epoch
- **Monthly**: windows start every 2,592,000 seconds from epoch (not calendar months)
- **Custom**: windows start every N seconds from epoch

This alignment ensures that all gateway instances compute the same window
boundaries for a given timestamp, without any coordination or shared clock.

### Counter TTL

When a counter is written, its TTL is set to the window duration. This means
counters automatically expire when their window closes, providing natural
cleanup without a background reaper.

---

## 3. Gateway Integration

### Pipeline Position

The quota check runs at step 2b in the dispatch pipeline:

```
1.  Acquire distributed lock
2b. check_quota(&action)        <-- Quota enforcement
3.  Build eval context + evaluate rules
3b. LLM guardrail
3c. Dry-run early return
4.  Handle verdict (execute, suppress, reroute, ...)
```

This position was chosen because:

- **After lock**: The lock prevents concurrent dispatches from racing on the
  counter increment (read + check + write is safe under lock).
- **Before rules**: Quota limits should override all rule logic. A tenant over
  quota should be blocked even if their action matches an allow rule.
- **Skipped in dry-run**: Dry-run mode bypasses the quota check (and the lock)
  so that rule testing is not affected by quotas.

### `check_quota()` Algorithm

```
1. Look up policy by key "namespace:tenant"
2. If no policy exists or policy is disabled, return None (allow)
3. Compute counter key from namespace, tenant, window, current time
4. Read current counter value from state store
5. If current < max_actions:
   a. Increment counter (set with window TTL)
   b. Return None (allow)
6. Counter >= max_actions -- apply overage behavior:
   a. Block: increment quota_exceeded metric, return QuotaExceeded outcome
   b. Warn: increment counter + quota_warned metric, return None (allow)
   c. Degrade: increment quota_degraded metric, return QuotaExceeded with
      fallback provider info
   d. Notify: increment counter + quota_notified metric, send notification,
      return None (allow)
```

### Policy Storage in Gateway

Policies are stored in a `HashMap<String, QuotaPolicy>` on the `Gateway` struct,
keyed by `"namespace:tenant"`. This provides O(1) lookup during dispatch.

At build time, policies are set via `GatewayBuilder::quota_policy()`. At runtime,
the API can add/update/remove policies via `Gateway::set_quota_policy()` and
`Gateway::remove_quota_policy()`.

---

## 4. Distributed Safety

### Counter Consistency Under Lock

The quota counter read-check-increment is performed while holding the per-action
distributed lock. This means:

- Only one dispatch for a given action ID can check the counter at a time
- The read-increment-write sequence is effectively atomic per action

However, **different action IDs** for the same tenant can race on the counter.
In the worst case, two concurrent dispatches both read the same counter value
and both increment, allowing the limit to be exceeded by 1. This is an accepted
trade-off for performance -- the alternative (a per-tenant lock) would serialize
all dispatches for a tenant.

### Cross-Instance Agreement

All instances compute the same counter key because:

1. Window boundaries are epoch-aligned (deterministic from the timestamp)
2. The counter key includes the window index (changes atomically at boundaries)
3. All instances share the same state backend

This means a counter incremented by instance A is immediately visible to
instance B on the next read.

### Window Boundary Transitions

When a window boundary is crossed, a new counter key is generated. The old
counter key is left to expire via its TTL. There is no explicit cleanup needed.

At the exact boundary instant, two instances might disagree on which window
they are in (if their clocks differ slightly). This is benign: at worst, a
single action near the boundary is counted in the wrong window.

---

## 5. Interaction with Rate Limiting

Quotas and rate limiting serve different purposes:

| Aspect | Quota | Rate Limit |
|--------|-------|------------|
| **Scope** | Per-tenant billing/usage | Per-endpoint request throttling |
| **Window** | Hours/days/weeks/months | Seconds/minutes |
| **Purpose** | Enforce subscription tier limits | Protect infrastructure |
| **Position** | Gateway dispatch pipeline | HTTP middleware (before dispatch) |
| **Counter** | Action count | Request count |
| **Shared state** | State backend (distributed) | In-memory or Redis |

A tenant that is under their quota can still be rate-limited if they send too
many requests per second. Conversely, a tenant within rate limits can still hit
their quota if they have dispatched too many actions over the billing window.

---

## 6. Metrics and Observability

### `GatewayMetrics` Fields

```rust
pub quota_exceeded: AtomicU64,   // Actions blocked (Block behavior)
pub quota_warned: AtomicU64,     // Actions warned (Warn behavior)
pub quota_degraded: AtomicU64,   // Actions degraded (Degrade behavior)
```

These counters are included in the `MetricsSnapshot` returned by
`Gateway::metrics().snapshot()` and exposed via the `GET /health` endpoint.

### Structured Logging

The `check_quota` method is instrumented with `#[instrument(name = "gateway.check_quota")]`
and emits:

- `info!` for Block and Degrade decisions (with tenant, limit, used)
- `warn!` for Warn decisions (with tenant, limit, used)
- `info!` for Notify decisions (with tenant, target)

### Audit Trail

When a quota check produces an outcome (Block or Degrade), an audit record is
written with the `QuotaExceeded` outcome before returning early from the
dispatch pipeline. This ensures quota enforcement is visible in the audit trail.

---

## 7. Module / File Layout

### Core types

```
crates/core/src/quota.rs          -- QuotaPolicy, QuotaUsage, QuotaWindow,
                                      OverageBehavior, compute_window_boundaries(),
                                      quota_counter_key()
crates/core/src/lib.rs            -- pub mod quota; re-exports
crates/core/src/outcome.rs        -- ActionOutcome::QuotaExceeded variant
```

### State keys

```
crates/state/state/src/key.rs     -- KeyKind::Quota, KeyKind::QuotaUsage
```

### Gateway

```
crates/gateway/src/gateway.rs     -- check_quota() method, quota_policies field,
                                      set_quota_policy(), remove_quota_policy()
crates/gateway/src/builder.rs     -- quota_policy(), quota_policies() builder methods
crates/gateway/src/metrics.rs     -- quota_exceeded, quota_warned, quota_degraded counters
```

### Server

```
crates/server/src/api/quotas.rs   -- CRUD + usage API handlers (when implemented)
crates/server/src/api/health.rs   -- Quota metrics in health snapshot
```

---

## 8. Summary of Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Pipeline position | After lock, before rules | Quotas override rules; lock protects counter |
| Window alignment | Epoch-aligned | All instances agree without coordination |
| Counter storage | State backend with TTL | Natural expiry, no reaper needed |
| Counter precision | Non-atomic (read + write) | Acceptable for billing (off-by-one at most) |
| Policy lookup | In-memory HashMap | O(1) per dispatch, policies rarely change |
| Per-tenant lock | Not used | Would serialize all dispatches per tenant |
| Overage behaviors | Block/Warn/Degrade/Notify | Covers common billing and ops scenarios |
