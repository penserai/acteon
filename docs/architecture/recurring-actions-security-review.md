# Recurring Actions: Security & Reliability Review

**Reviewer:** security-advocate
**Date:** 2026-02-09
**Architecture doc:** `docs/architecture/recurring-actions.md`

---

## Methodology

This review evaluates the recurring actions architecture against the existing
Acteon security and reliability patterns found in:

- `crates/gateway/src/background.rs` -- scheduled action claim pattern
- `crates/state/state/src/store.rs` -- `StateStore` trait, `check_and_set` atomicity
- `crates/state/state/src/lock.rs` -- `DistributedLock` / `LockGuard` traits
- `crates/server/src/config.rs` -- limit configuration patterns
- `crates/gateway/src/metrics.rs` -- metric counter patterns
- `crates/server/src/api/dispatch.rs` -- request validation, tenant isolation, rate limiting
- `crates/server/src/ratelimit/` -- sliding-window rate limiter, per-tenant/caller tiers

Findings are categorized as:

- **MUST-FIX** -- Blocks implementation; must be resolved before merging.
- **SHOULD-FIX** -- Important but can be addressed in a fast follow-up.
- **NICE-TO-HAVE** -- Low priority; improves the feature but not critical.

---

## Security Findings

### S1. No per-tenant limit on recurring action count [MUST-FIX]

**Risk:** Resource exhaustion / denial-of-service.

The architecture defines CRUD endpoints but does not specify a maximum number of
recurring actions per tenant. A single tenant (or a compromised API key) could
create millions of recurring actions, each adding a timeout index entry. This
would:

1. Exhaust state store memory/disk (especially with the memory backend).
2. Degrade `get_expired_timeouts()` performance for all tenants since the
   timeout index is global.
3. Generate an unbounded fan-out of dispatches on each poll cycle.

**Recommendation:** Add a configurable `max_recurring_actions_per_tenant` limit
(default: 1000) to the `BackgroundProcessingConfig` or a new
`RecurringActionsConfig` section. Enforce it in the `POST /v1/recurring` handler
by counting existing `RecurringAction` keys for the tenant via `scan_keys()`
before creation.

```toml
[recurring]
max_per_tenant = 1000
```

The count check does not need to be atomic (a small race is acceptable since the
worst case is slightly exceeding the limit).

---

### S2. Cron expression validation must be strict and bounded [MUST-FIX]

**Risk:** Cron expression injection / excessive firing frequency.

The architecture states `cron_expr` "must parse as a valid cron expression" but
does not specify:

1. **Minimum interval enforcement.** A cron expression like `* * * * *` (every
   minute) or `*/1 * * * * *` (every second, if 6-field is allowed) could
   generate an overwhelming number of dispatches. Combined with no per-tenant
   limit (S1), this is a force multiplier.

2. **Expression complexity limits.** The `croner` crate supports 5/6/7-field
   expressions. Allowing second-level granularity (6-field) could enable very
   high-frequency schedules. The architecture should decide whether seconds-level
   scheduling is permitted.

3. **Input sanitization.** While `croner` is a parser (not an interpreter that
   executes strings), the raw cron expression is stored in the state store and
   displayed in the admin UI. Ensure that the expression is validated (parsed)
   server-side and that the stored value matches the parsed canonical form to
   prevent stored XSS via the admin UI if the expression is rendered unsanitized.

**Recommendation:**

- Enforce a configurable minimum interval (default: 60 seconds). After parsing
  the cron expression, compute two consecutive occurrences and reject if
  `next2 - next1 < min_interval`. This prevents sub-minute schedules.
- Decide whether to restrict to 5-field cron only (minute granularity) or allow
  6-field with the minimum interval guard. Document the decision.
- Validate the expression server-side before storing. Store the validated
  expression (not raw user input).

---

### S3. Tenant isolation on API endpoints [MUST-FIX]

**Risk:** Cross-tenant data access.

The architecture specifies namespace and tenant as query parameters. The
existing `dispatch.rs` handler checks `CallerIdentity.is_authorized()` against
the action's tenant, namespace, and action_type. The recurring action endpoints
**must** enforce the same grant-level authorization.

Current risk: The architecture does not explicitly mention auth/permission
checks on the CRUD endpoints. Without these:

- A caller with access to tenant A could list/update/delete recurring actions
  belonging to tenant B.
- The `pause` and `resume` endpoints could be used to interfere with another
  tenant's schedules.

**Recommendation:** Every recurring action endpoint must:

1. Require `Permission::Dispatch` (or a new `Permission::ManageRecurring`)
   role permission.
2. Call `identity.is_authorized(tenant, namespace, action_type)` using the
   recurring action's template `action_type` (for create/update) or the stored
   action's fields (for read/delete/pause/resume).
3. Return 403 if authorization fails.

Follow the pattern in `crates/server/src/api/dispatch.rs` lines 55-75.

---

### S4. Rate limiting on CRUD endpoints [SHOULD-FIX]

**Risk:** Abuse of management plane.

The dispatch endpoint has per-tenant and per-caller rate limiting. The recurring
action CRUD endpoints should also be rate-limited, especially `POST` (create)
and `PUT` (update). Without rate limits, an attacker could:

- Rapidly create/delete recurring actions to cause state store churn.
- Use the `PUT` endpoint to rapidly change cron expressions, causing the
  background processor to recompute and re-index on every poll.

**Recommendation:** Apply the existing `RateLimitLayer` middleware to the
recurring action routes (it already covers `/v1/*` protected routes). Optionally
add a tighter custom rate limit on the `POST /v1/recurring` endpoint using the
`RateLimiter::check_custom_limit()` method (e.g., 100 creates per minute per
tenant).

---

### S5. Audit trail for recurring action lifecycle events [SHOULD-FIX]

**Risk:** Incomplete audit trail.

The architecture mentions structured logging for lifecycle events
(`recurring.create`, `recurring.delete`, etc.) but does not mention audit store
integration. In a multi-tenant system, lifecycle changes (who created a
recurring action, who paused it, who changed the cron expression) should be
auditable.

**Recommendation:** Emit `AuditRecord` entries for create, update, delete,
pause, and resume operations. Include the `CallerIdentity` as the actor. The
dispatched actions themselves will be audited through the normal gateway pipeline
(this is already covered).

---

### S6. Dedup key template injection [NICE-TO-HAVE]

**Risk:** Low -- dedup key collisions across tenants.

The `dedup_key` template supports `{{recurring_id}}` and
`{{execution_time}}` placeholders. The architecture should clarify that the
dedup key is scoped to the recurring action's namespace and tenant (via the
`StateKey` structure). Since `StateKey` already includes namespace and tenant,
cross-tenant dedup collisions are not possible.

However, if a user crafts a `dedup_key` that matches another action's dedup key
within the same tenant, it could cause unintended dedup suppression.

**Recommendation:** Document that dedup keys are namespace:tenant-scoped and
that users are responsible for ensuring uniqueness within their tenant. No code
change needed.

---

## Reliability Findings

### R1. At-most-once vs at-least-once semantics are unclear [MUST-FIX]

**Risk:** Duplicate dispatches or missed dispatches.

The architecture uses `check_and_set` for the claim key (at-most-once per
claim attempt) but the crash recovery section describes a scenario where the
claim expires and the action is re-dispatched. This creates a subtle hybrid:

- **Normal case:** At-most-once per occurrence (CAS ensures single winner).
- **Crash case:** At-most-once for the crashed attempt, but the next poll
  re-dispatches (potentially at-least-once for the occurrence).

The `last_executed_at` check described in section 7 ("if `last_executed_at` is
already within the current cron window, skip") is critical for preventing
double-dispatch after a crash. However, this check is **not present** in the
step-by-step algorithm in section 3.

**Recommendation:** Add an explicit idempotency check in step 5d of the
algorithm:

```
d. Check: if last_executed_at is within the current cron window, skip
   (another instance already dispatched this occurrence).
```

Also ensure the `last_executed_at` update (step 5i) happens **before** emitting
the event (step 5k) to minimize the double-dispatch window. Consider using
`compare_and_swap` on the `RecurringAction` definition (with versioning) instead
of plain `set` to detect concurrent updates.

---

### R2. Clock skew in distributed deployments [SHOULD-FIX]

**Risk:** Missed or premature dispatches.

The background processor uses `Utc::now().timestamp_millis()` to query expired
timeouts. In a distributed deployment, clock skew between instances can cause:

1. **Premature dispatch:** An instance with a fast clock dispatches before the
   actual scheduled time.
2. **Delayed dispatch:** An instance with a slow clock doesn't see the key as
   expired until skew resolves.
3. **Double dispatch:** Instance A (fast clock) claims and dispatches at T-2s.
   Instance B (slow clock) doesn't see the claim yet (CAS check happens at
   T-2s on instance A but instance B's clock is at T-4s, so the claim key
   doesn't exist in its view yet).

The 60-second CAS TTL provides a large buffer -- typical NTP skew is under 1
second. The existing scheduled action processor has the same exposure, so this
is not a new risk, but it should be documented.

**Recommendation:** Document the clock skew assumption (NTP-synchronized nodes
with < 1s skew). Add a configurable `clock_skew_tolerance_ms` to the background
config (default: 0, advanced tuning only) that is subtracted from `now_ms`
before querying expired timeouts. This provides a safety margin at the cost of
slightly delayed dispatch.

---

### R3. State backend failure modes during schedule advancement [SHOULD-FIX]

**Risk:** Orphaned or stuck recurring actions.

The algorithm in section 3 has multiple state store operations between steps 5g
and 5j:

```
g. Remove old pending index + timeout entry
h. Compute next_execution_at
i. Update RecurringAction definition
j. Re-index with new pending key + timeout
```

If the process crashes between (g) and (j), the recurring action loses its
pending index entry and will never fire again. The definition still exists but
has no timeout index entry.

The scheduled action processor has the same pattern but it doesn't need to
re-index (one-shot actions are done after dispatch). Recurring actions are
unique in that they need to atomically swap the old index for a new one.

**Recommendation:** Reverse the order of operations:

1. First, write the new pending index entry (step j) with the next occurrence.
2. Then, remove the old pending index entry (step g).
3. Then, update the definition (step i).

This way, if a crash occurs:
- After step 1, before step 2: Two pending entries exist. The old one triggers
  a re-dispatch attempt, but the `last_executed_at` idempotency check (R1)
  skips it. The new one fires at the correct time.
- After step 2, before step 3: Pending index is correct. Definition has stale
  `last_executed_at` but the next poll will see the new pending entry.

Alternatively, implement a repair/reconciliation sweep in the cleanup interval
that scans `RecurringAction` definitions and ensures each enabled one has a
corresponding `PendingRecurring` index entry.

---

### R4. Backpressure when dispatch takes longer than interval [SHOULD-FIX]

**Risk:** Unbounded goroutine/task spawn, memory exhaustion.

The consumer wiring in section 12 spawns a new `tokio::spawn` for each
`RecurringActionDueEvent`. If a tenant has many recurring actions firing
simultaneously (e.g., 1000 actions at minute boundaries), this spawns 1000
concurrent tasks. Each task calls `gateway.dispatch()` which may involve
HTTP calls to providers, LLM evaluation, etc.

The existing chain advancement uses `max_concurrent_advances` (default: 16) for
backpressure. Recurring actions need a similar mechanism.

**Recommendation:** Use a `tokio::sync::Semaphore` in the consumer to limit
concurrent recurring action dispatches. Add a configurable
`max_concurrent_recurring_dispatches` (default: 16) to the background config
or recurring config. The channel buffer size should also be bounded (it is,
via `mpsc::channel(N)` -- specify N explicitly in the architecture, e.g., 256).

---

### R5. Circuit breaker interaction [NICE-TO-HAVE]

**Risk:** Silent failures when provider is down.

When a recurring action fires and the target provider's circuit breaker is open,
the action will be rejected with `ActionOutcome::CircuitOpen`. The recurring
action processor does not retry this occurrence (no backfill policy).

This means that if a provider is down for an extended period, all recurring
actions targeting it will silently miss their occurrences. The `execution_count`
won't increment, providing some observability, but there's no alerting path.

**Recommendation:** When a recurring action dispatch returns
`ActionOutcome::CircuitOpen`, increment a dedicated
`recurring_circuit_open_skips` metric counter and log at warn level. Consider
adding an optional `retry_on_circuit_open` boolean to `RecurringAction` that
would re-index with a short delay (e.g., 30 seconds) instead of skipping to
the next cron occurrence.

---

### R6. Monitoring gaps [SHOULD-FIX]

**Risk:** Operational blindness.

The architecture adds three metric counters: `recurring_dispatched`,
`recurring_active`, `recurring_errors`. This is a good start but has gaps:

1. **No latency metric.** How long does it take from `next_execution_at` to
   actual dispatch? This is the scheduling jitter/lag and is critical for
   observability.
2. **No per-tenant breakdown.** All counters are global. In a multi-tenant
   deployment, it's important to know which tenant's recurring actions are
   generating the most load.
3. **`recurring_active` semantics are unclear.** The architecture says "set on
   poll" but `AtomicU64` only supports increment. To track a gauge-like
   "active count", you need to load-and-store or use a different mechanism.

**Recommendation:**

- Add a `recurring_dispatch_lag_ms` histogram or, at minimum, log the lag
  (`Utc::now() - next_execution_at`) in each dispatch event.
- For the `recurring_active` gauge, use `AtomicU64::store()` (set, not
  increment) to update the count during each poll cycle after scanning.
  Alternatively, query the count from the state store via the list endpoint.
- Per-tenant metrics can be deferred (the existing `GatewayMetrics` pattern is
  global), but document that this is a known gap for multi-tenant monitoring.

---

### R7. Timeout index shared with other key kinds [NICE-TO-HAVE]

**Risk:** Performance degradation under high load.

The `PendingRecurring` keys share the timeout index (`BTreeMap` or Redis sorted
set) with `EventTimeout` and `PendingScheduled` keys. The background processor
queries all expired keys and then filters by key segment.

In a deployment with many event timeouts (e.g., thousands of state machine
timers), the recurring action processor pays the cost of scanning all expired
keys even though most belong to `EventTimeout`. This is the same trade-off that
`PendingScheduled` already makes.

**Recommendation:** For the initial implementation, this is acceptable. Document
the performance characteristic. If it becomes a bottleneck, consider adding a
dedicated sorted index for recurring actions (similar to the
`index_chain_ready` / `get_ready_chains` pattern for chains).

---

### R8. No health check for "stuck" recurring actions [NICE-TO-HAVE]

**Risk:** Recurring actions that silently stop firing.

A recurring action could become "stuck" (enabled but not firing) if:
- Its `PendingRecurring` index entry is lost (see R3).
- The cron expression has no future occurrences (though the architecture
  handles this case).
- A bug in the `croner` crate causes parsing to succeed but
  `find_next_occurrence` to return `None`.

**Recommendation:** Add a reconciliation check to the cleanup interval. For
each enabled `RecurringAction` whose `next_execution_at` is more than
`2 * cron_interval` in the past, verify that a `PendingRecurring` index entry
exists. If not, re-index it. Log a warning when reconciliation repairs an
entry.

---

## Summary

| # | Category | Severity | Summary |
|---|----------|----------|---------|
| S1 | Security | **MUST-FIX** | No per-tenant limit on recurring action count |
| S2 | Security | **MUST-FIX** | Cron expression validation needs minimum interval and complexity bounds |
| S3 | Security | **MUST-FIX** | Tenant isolation (auth/grant checks) not specified on CRUD endpoints |
| S4 | Security | SHOULD-FIX | Rate limiting on CRUD endpoints |
| S5 | Security | SHOULD-FIX | Audit trail for lifecycle events |
| S6 | Security | NICE-TO-HAVE | Dedup key template scoping documentation |
| R1 | Reliability | **MUST-FIX** | Idempotency check (`last_executed_at`) missing from algorithm |
| R2 | Reliability | SHOULD-FIX | Clock skew documentation and optional tolerance |
| R3 | Reliability | SHOULD-FIX | Crash between index removal and re-index causes stuck actions |
| R4 | Reliability | SHOULD-FIX | No backpressure on concurrent recurring dispatches |
| R5 | Reliability | NICE-TO-HAVE | Circuit breaker interaction causes silent missed occurrences |
| R6 | Reliability | SHOULD-FIX | Monitoring gaps (latency, gauge semantics, per-tenant) |
| R7 | Reliability | NICE-TO-HAVE | Shared timeout index performance under high load |
| R8 | Reliability | NICE-TO-HAVE | No reconciliation check for stuck recurring actions |

**MUST-FIX count: 4** (S1, S2, S3, R1)
**SHOULD-FIX count: 5** (S4, S5, R2, R3, R4, R6)
**NICE-TO-HAVE count: 4** (S6, R5, R7, R8)

The architecture is well-designed and follows established patterns (CAS claim,
timeout index, channel events). The main gaps are around multi-tenant resource
limits (S1, S2), explicit auth enforcement (S3), and crash-resilient schedule
advancement (R1, R3). Addressing the MUST-FIX items before implementation will
prevent security and correctness regressions.
