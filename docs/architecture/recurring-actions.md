# Recurring Actions Architecture

## Overview

Recurring actions allow users to define an action that fires on a cron schedule.
This turns Acteon into a lightweight scheduler for recurring notifications (daily
digests, weekly reports, periodic health checks).

Recurring actions are API-only entities -- they are **not** created via the rule
engine. A recurring action is a stored definition; each time its cron expression
fires, the background processor synthesizes a concrete `Action` and dispatches it
through the normal gateway pipeline (rules, dedup, providers, audit, etc.).

---

## 1. Data Model

### `RecurringAction` (core struct, lives in `acteon-core`)

```rust
/// A recurring action definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringAction {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Namespace this recurring action belongs to.
    pub namespace: String,
    /// Tenant that owns this recurring action.
    pub tenant: String,
    /// Cron expression (standard 5-field or extended 6/7-field).
    /// Examples: "0 9 * * MON-FRI", "*/5 * * * *"
    pub cron_expr: String,
    /// IANA timezone for evaluating the cron expression.
    /// Defaults to "UTC" if not provided.
    pub timezone: String,
    /// Whether this recurring action is currently active.
    pub enabled: bool,
    /// The action template dispatched on each occurrence.
    pub action_template: RecurringActionTemplate,
    /// When this recurring action was created.
    pub created_at: DateTime<Utc>,
    /// When this recurring action was last updated.
    pub updated_at: DateTime<Utc>,
    /// The most recent execution time (None if never executed).
    pub last_executed_at: Option<DateTime<Utc>>,
    /// The next scheduled execution time (None if paused or expired).
    pub next_execution_at: Option<DateTime<Utc>>,
    /// Optional end date after which the recurring action is auto-disabled.
    pub ends_at: Option<DateTime<Utc>>,
    /// Total number of successful executions.
    pub execution_count: u64,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Arbitrary key-value labels for filtering and organization.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}
```

### `RecurringActionTemplate` (the action to dispatch)

```rust
/// Template for the action dispatched on each cron tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringActionTemplate {
    /// Target provider (e.g. "email", "webhook").
    pub provider: String,
    /// Action type discriminator (e.g. "send_digest").
    pub action_type: String,
    /// JSON payload for the provider.
    pub payload: serde_json::Value,
    /// Optional metadata labels merged into each dispatched action.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Optional dedup key template. Supports `{{recurring_id}}` and
    /// `{{execution_time}}` placeholders.
    pub dedup_key: Option<String>,
}
```

### `ActionOutcome::RecurringCreated` (new variant)

```rust
/// Action outcome when a recurring action is created via the API.
RecurringCreated {
    /// The recurring action ID.
    recurring_id: String,
    /// When the first execution is scheduled.
    next_execution: DateTime<Utc>,
}
```

---

## 2. State Storage Design

### New `KeyKind` Variants

| KeyKind | `as_str()` | Purpose |
|---------|-----------|---------|
| `RecurringAction` | `"recurring_action"` | Stores the `RecurringAction` JSON definition |
| `PendingRecurring` | `"pending_recurring"` | Timeout-indexed key for efficient polling |

### Key Layout

| Key | Format | TTL |
|-----|--------|-----|
| **Definition** | `{ns}:{tenant}:recurring_action:{id}` | None (permanent until deleted) |
| **Pending index** | `{ns}:{tenant}:pending_recurring:{id}` | None (managed by processor) |
| **Claim** | `{ns}:{tenant}:recurring_action:{id}:claim` | 60s (auto-expire) |
| **Execution log** (optional) | `{ns}:{tenant}:recurring_action:{id}:exec:{timestamp}` | 7 days |

### Index Strategy

The `PendingRecurring` key is indexed in the **timeout index** (same `BTreeMap`
used by `EventTimeout` and `PendingScheduled`). The indexed timestamp is
`next_execution_at` in epoch milliseconds.

When the background processor polls `get_expired_timeouts(now_ms)`, it filters
results by the `:pending_recurring:` key segment -- identical to how
`process_scheduled_actions()` filters by `:pending_scheduled:`.

After dispatching a recurring action:
1. Remove the old timeout index entry.
2. Compute the next occurrence from the cron expression.
3. Re-index with the new `next_execution_at`.
4. Update the `RecurringAction` definition in the state store with the new
   `next_execution_at`, `last_executed_at`, and incremented `execution_count`.

This reuse means no new `StateStore` trait methods are required.

---

## 3. Background Processor Flow

### New `BackgroundConfig` fields

```rust
/// Whether recurring action processing is enabled (default: false).
pub enable_recurring_actions: bool,
/// How often to check for due recurring actions (default: 5 seconds).
pub recurring_check_interval: Duration,
```

### New event type

```rust
/// Event emitted when a recurring action is due for dispatch.
#[derive(Debug, Clone)]
pub struct RecurringActionDueEvent {
    /// Namespace.
    pub namespace: String,
    /// Tenant.
    pub tenant: String,
    /// The recurring action ID.
    pub recurring_id: String,
    /// The full recurring action definition.
    pub recurring_action: RecurringAction,
    /// The concrete action to dispatch.
    pub action: Action,
}
```

### Step-by-step algorithm (`process_recurring_actions`)

```
1. Guard: return early if channel is None
2. now_ms = Utc::now().timestamp_millis()
3. expired_keys = state.get_expired_timeouts(now_ms)
4. due_keys = expired_keys.filter(|k| k.contains(":pending_recurring:"))
5. For each due_key:
   a. Parse namespace, tenant, recurring_id from key
   b. Atomically claim: check_and_set(claim_key, "claimed", TTL=60s)
      - If not claimed, skip (another instance is handling it)
   c. Load RecurringAction definition from state store
      - If missing, clean up pending index and continue
   d. Check: if !enabled, skip (but leave index for resume)
   e. Check: if ends_at is set and ends_at <= now, disable and clean up
   f. Build concrete Action from action_template:
      - Generate new UUID for action.id
      - Set namespace, tenant, provider, action_type, payload from template
      - Set metadata: merge template metadata + {"_recurring_dispatch": "true",
        "_recurring_id": recurring_id}
      - Expand dedup_key template if present
   g. Remove old pending index + timeout entry
   h. Compute next_execution_at from cron expression + timezone
   i. Update RecurringAction in state store:
      - last_executed_at = now
      - next_execution_at = computed next
      - execution_count += 1
   j. Re-index: set new pending key, index_timeout(pending_key, next_ms)
   k. Emit RecurringActionDueEvent to channel
6. Log dispatched count
```

### No backfill policy

If the server was down and missed N occurrences, only the **next future
occurrence** is scheduled when the processor runs. This prevents a storm of
catch-up dispatches. The `last_executed_at` field provides observability into
missed windows.

### Dispatched action loop prevention

The concrete action's metadata includes `_recurring_dispatch: true`. The
consumer that receives `RecurringActionDueEvent` should dispatch the action
through the gateway with a flag to bypass the `Schedule` rule action (similar to
how `ScheduledActionDueEvent` prevents re-scheduling via `_scheduled_dispatch`).

---

## 4. API Endpoint Specifications

All endpoints live under `/v1/recurring`. Namespace and tenant are provided as
query parameters (consistent with chains, approvals, etc.).

### `POST /v1/recurring` -- Create recurring action

**Request body:**
```json
{
  "namespace": "notifications",
  "tenant": "tenant-1",
  "cron_expr": "0 9 * * MON-FRI",
  "timezone": "US/Eastern",
  "provider": "email",
  "action_type": "send_digest",
  "payload": {"to": "team@example.com", "subject": "Daily Digest"},
  "metadata": {},
  "dedup_key": null,
  "description": "Weekday morning digest",
  "labels": {"team": "engineering"},
  "ends_at": null,
  "enabled": true
}
```

**Response (201):**
```json
{
  "recurring_id": "uuid-...",
  "next_execution": "2026-02-10T14:00:00Z",
  "cron_expr": "0 9 * * MON-FRI",
  "timezone": "US/Eastern",
  "enabled": true
}
```

**Validation:**
- `cron_expr` must parse as a valid cron expression.
- `timezone` must be a valid IANA timezone (parsed via `chrono-tz`).
- `provider` and `action_type` must be non-empty strings.
- `payload` must be a valid JSON object.
- If `ends_at` is provided, it must be in the future.

### `GET /v1/recurring` -- List recurring actions

**Query params:** `namespace` (required), `tenant` (required), `enabled` (optional bool filter)

**Response (200):**
```json
{
  "recurring_actions": [
    {
      "id": "uuid-...",
      "cron_expr": "0 9 * * MON-FRI",
      "timezone": "US/Eastern",
      "enabled": true,
      "description": "Weekday morning digest",
      "provider": "email",
      "action_type": "send_digest",
      "next_execution_at": "2026-02-10T14:00:00Z",
      "last_executed_at": "2026-02-07T14:00:00Z",
      "execution_count": 42,
      "created_at": "2026-01-01T00:00:00Z",
      "labels": {"team": "engineering"}
    }
  ]
}
```

### `GET /v1/recurring/{id}` -- Get recurring action detail

**Query params:** `namespace`, `tenant`

**Response (200):** Full `RecurringAction` JSON.

**Response (404):** `{"error": "recurring action not found"}`

### `PUT /v1/recurring/{id}` -- Update recurring action

**Query params:** `namespace`, `tenant`

**Request body:** Partial update. Only provided fields are changed. Updatable
fields: `cron_expr`, `timezone`, `enabled`, `payload`, `metadata`, `dedup_key`,
`description`, `labels`, `ends_at`, `provider`, `action_type`.

On update:
- If `cron_expr` or `timezone` changed, recompute `next_execution_at` and
  re-index the pending key.
- `updated_at` is always set to now.

**Response (200):** Updated `RecurringAction` JSON.

### `DELETE /v1/recurring/{id}` -- Delete recurring action

**Query params:** `namespace`, `tenant`

Deletes the definition, pending index, and timeout index entry.

**Response (204):** No content.

### `POST /v1/recurring/{id}/pause` -- Pause recurring action

**Query params:** `namespace`, `tenant`

Sets `enabled = false`, removes the pending index entry. Does not delete the
definition.

**Response (200):**
```json
{"id": "uuid-...", "enabled": false, "next_execution_at": null}
```

### `POST /v1/recurring/{id}/resume` -- Resume recurring action

**Query params:** `namespace`, `tenant`

Sets `enabled = true`, computes the next occurrence from now, re-indexes.

**Response (200):**
```json
{"id": "uuid-...", "enabled": true, "next_execution_at": "2026-02-10T14:00:00Z"}
```

### `GET /v1/recurring/{id}/history` -- Execution history (optional/future)

Returns the last N execution records with timestamps and outcomes.
Deferred to a follow-up iteration.

---

## 5. Server Configuration Schema

### TOML config (`acteon.toml`)

```toml
[background]
# Existing fields...
enable_recurring_actions = false
recurring_check_interval_seconds = 5
```

### `BackgroundProcessingConfig` additions

```rust
/// Whether to process recurring actions.
#[serde(default)]
pub enable_recurring_actions: bool,

/// How often to check for due recurring actions (seconds).
#[serde(default = "default_recurring_check_interval")]
pub recurring_check_interval_seconds: u64,
```

Default: `recurring_check_interval_seconds = 5`.

### `BackgroundSnapshot` additions

```rust
/// Whether recurring actions are enabled.
pub enable_recurring_actions: bool,
/// Recurring action check interval in seconds.
pub recurring_check_interval_seconds: u64,
```

---

## 6. Error Handling Strategy

| Error condition | Handling |
|----------------|----------|
| Invalid cron expression | 400 Bad Request on API create/update |
| Invalid timezone | 400 Bad Request on API create/update |
| Cron has no next occurrence | Disable the recurring action, log warning |
| State store failure during poll | Log error, retry on next interval |
| Claim contention (CAS fails) | Skip, another instance handles it |
| Action dispatch fails | Log error; do NOT retry the occurrence (no backfill). The recurring action remains active for the next tick. |
| Definition not found during poll | Clean up orphaned index entries |
| `ends_at` in the past on create | 400 Bad Request |
| `ends_at` reached during poll | Disable the recurring action, remove pending index |

### Graceful degradation

- If the cron crate cannot compute a next occurrence (e.g., impossible schedule
  like Feb 30), the recurring action is disabled with a state-stored error
  message.
- If dispatch channel is full, the event is dropped and the recurring action
  will fire again on the next tick (at-least-once for future occurrences).

---

## 7. Distributed Coordination Approach

Recurring actions use the same `check_and_set` claim pattern as scheduled
actions:

1. When the background processor finds a due `PendingRecurring` key, it
   attempts `check_and_set(claim_key, "claimed", TTL=60s)`.
2. Only one instance wins the CAS. Losers skip silently.
3. The winner dispatches the action and advances the schedule.
4. The claim key auto-expires after 60 seconds, providing crash recovery.

### Crash recovery

If the winner crashes after claiming but before re-indexing:
- The claim key expires after 60s.
- The `PendingRecurring` index still has the old (past-due) timestamp.
- On the next poll, the key shows as expired again. Another instance claims it.
- The no-backfill policy means only the next future occurrence is indexed
  (not the missed one). The `last_executed_at` check prevents double-dispatch:
  if `last_executed_at` is already within the current cron window, skip.

### Leader election (not needed)

The timeout-index + CAS claim pattern provides coordination without leader
election. All instances poll independently; the CAS ensures at-most-once
dispatch per occurrence.

---

## 8. Metrics and Observability

### New `GatewayMetrics` fields

```rust
/// Recurring actions dispatched.
pub recurring_dispatched: AtomicU64,
/// Recurring actions that are currently active (gauge-like, set on poll).
pub recurring_active: AtomicU64,
/// Recurring action dispatch errors.
pub recurring_errors: AtomicU64,
```

### New `MetricsSnapshot` fields

```rust
pub recurring_dispatched: u64,
pub recurring_active: u64,
pub recurring_errors: u64,
```

### Structured logging

All recurring action operations emit `tracing` spans and events:

- `recurring.create` -- new recurring action created
- `recurring.update` -- recurring action updated
- `recurring.delete` -- recurring action deleted
- `recurring.pause` / `recurring.resume`
- `recurring.dispatch` -- occurrence dispatched (info level)
- `recurring.skip_claimed` -- another instance claimed (debug level)
- `recurring.expired` -- ends_at reached, auto-disabled (warn level)
- `recurring.no_next` -- cron has no future occurrence (warn level)
- `recurring.dispatch_error` -- dispatch failed (error level)

### Admin UI

The admin UI page should display:
- List of recurring actions with status, next/last execution
- Cron expression in human-readable form (e.g., "Every weekday at 9:00 AM ET")
- Pause/resume/delete controls
- Execution count and last execution time

---

## 9. Module / File Layout

### New files

```
crates/core/src/recurring.rs          -- RecurringAction, RecurringActionTemplate structs
crates/gateway/src/recurring.rs       -- process_recurring_actions() helper (called by background.rs)
crates/server/src/api/recurring.rs    -- CRUD API handlers
ui/src/pages/RecurringActionsPage.tsx  -- Admin UI page
```

### Modified files

```
crates/core/src/lib.rs                -- pub mod recurring; re-export types
crates/core/src/outcome.rs            -- Add ActionOutcome::RecurringCreated variant
crates/state/state/src/key.rs         -- Add KeyKind::RecurringAction, KeyKind::PendingRecurring
crates/gateway/src/background.rs      -- Add recurring_action_tx channel, recurring interval,
                                          process_recurring_actions() call in select loop,
                                          RecurringActionDueEvent struct
crates/gateway/src/metrics.rs         -- Add recurring_dispatched, recurring_active, recurring_errors
crates/server/src/config.rs           -- Add enable_recurring_actions, recurring_check_interval_seconds
                                          to BackgroundProcessingConfig and BackgroundSnapshot
crates/server/src/api/mod.rs          -- Register recurring routes
crates/server/src/api/openapi.rs      -- Register recurring schemas
crates/server/src/main.rs             -- Wire recurring channel consumer (dispatch through gateway)
```

### Cargo.toml changes

Add `croner` crate as a workspace dependency. `croner` is chosen over the `cron`
crate because:
- It supports 5-field (standard), 6-field (with seconds), and 7-field (with
  years) cron expressions.
- It has timezone-aware next-occurrence computation via `chrono-tz`.
- It is actively maintained and has a clean API.
- It supports `#[no_std]` making it suitable for embedded or WASM use if needed
  in the future.

```toml
# In workspace Cargo.toml [workspace.dependencies]
croner = "2"
```

The `croner` dependency is added to `acteon-core` (for validation in the struct)
and `acteon-gateway` (for computing next occurrences in the background
processor).

---

## 10. Cron Crate Selection Rationale

| Crate | Stars | Last update | Timezone support | 6/7-field | API quality |
|-------|-------|-------------|-----------------|-----------|-------------|
| `cron` | ~600 | 2024 | No (UTC only) | No | Iterator-based |
| `croner` | ~100 | 2025 | Yes (chrono-tz) | Yes | `Cron::find_next_occurrence()` |
| `cronparse` | <50 | 2023 | No | No | Parse-only |

**Decision: `croner`**. Its timezone support is critical for recurring actions
(users expect "9 AM Eastern" to mean 9 AM Eastern year-round, including across
DST transitions). The `cron` crate would require manual timezone conversion.

Usage pattern:
```rust
use croner::Cron;
use chrono::Utc;
use chrono_tz::Tz;

let cron = Cron::new("0 9 * * MON-FRI").parse()?;
let tz: Tz = "US/Eastern".parse()?;
let next = cron.find_next_occurrence(&Utc::now(), tz)?;
```

---

## 11. Gateway Integration Decision

Recurring actions are **API-only** -- there is no `RuleAction::Recurring`
variant. Rationale:

1. **Separation of concerns**: Rules evaluate per-action conditions at dispatch
   time. Recurring actions are time-driven definitions that exist independently
   of any specific action dispatch.
2. **Lifecycle management**: Recurring actions need CRUD operations (create,
   pause, resume, delete) which don't fit the rule evaluation model.
3. **Simplicity**: Adding a rule action would require the rule engine to store
   state (the recurring definition), which violates its stateless evaluation
   model.

When a recurring action fires, the synthesized `Action` is dispatched through
the gateway's normal `dispatch()` method. This means all existing rules, dedup,
LLM guardrails, circuit breakers, etc. apply to each occurrence.

---

## 12. Consumer Wiring (in `main.rs`)

The consumer of `RecurringActionDueEvent` follows the same pattern as the
scheduled action consumer:

```rust
// In the background task consumer loop:
while let Some(event) = recurring_rx.recv().await {
    let gateway = Arc::clone(&gateway);
    let state = Arc::clone(&state);
    tokio::spawn(async move {
        // Mark the action to prevent re-scheduling loops.
        let mut action = event.action;
        if let Some(obj) = action.payload.as_object_mut() {
            obj.insert("_recurring_dispatch".into(), serde_json::Value::Bool(true));
        }

        // Dispatch through the gateway pipeline.
        match gateway.dispatch(action, /* dry_run */ false, None).await {
            Ok(outcome) => {
                info!(recurring_id = %event.recurring_id, ?outcome, "recurring action dispatched");
            }
            Err(e) => {
                error!(recurring_id = %event.recurring_id, error = %e, "recurring action dispatch failed");
            }
        }

        // Clean up the scheduled action data key (at-least-once delivery complete).
        // The recurring definition is NOT deleted -- it persists for the next tick.
    });
}
```

---

## 13. Sequence Diagram

```
User                API Server           State Store          Background Processor
 |                      |                     |                       |
 |-- POST /v1/recurring -->                   |                       |
 |                      |-- set(recurring_action, def) -->            |
 |                      |-- set(pending_recurring, next_ms) -->       |
 |                      |-- index_timeout(pending_recurring, ms) -->  |
 |                      |                     |                       |
 |<-- 201 Created ------|                     |                       |
 |                      |                     |                       |
 |                      |                     |    [poll interval]    |
 |                      |                     |<-- get_expired_timeouts
 |                      |                     |-- [pending_recurring due] -->
 |                      |                     |                       |
 |                      |                     |<-- check_and_set(claim)
 |                      |                     |-- true (claimed) ---->
 |                      |                     |                       |
 |                      |                     |<-- get(recurring_action)
 |                      |                     |-- RecurringAction --->
 |                      |                     |                       |
 |                      |                     |   [build Action from template]
 |                      |                     |                       |
 |                      |                     |<-- delete pending idx |
 |                      |                     |<-- remove_timeout_idx |
 |                      |                     |<-- compute next occurrence
 |                      |                     |<-- set pending (new ms)
 |                      |                     |<-- index_timeout (new ms)
 |                      |                     |<-- update recurring_action (last/next/count)
 |                      |                     |                       |
 |                      |    [RecurringActionDueEvent via channel]    |
 |                      |<--------------------------------------------|
 |                      |                     |                       |
 |                      |-- gateway.dispatch(action) -->              |
 |                      |   [rules, dedup, providers, audit, ...]     |
```

---

## Summary of Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Gateway integration | API-only (no RuleAction variant) | Recurring actions are time-driven, not condition-driven |
| Cron library | `croner` | Timezone support, 5/6/7-field, actively maintained |
| State storage | Reuse timeout index | Zero new StateStore trait methods |
| Distributed coordination | CAS claim (60s TTL) | Same proven pattern as scheduled actions |
| Missed executions | No backfill | Prevents dispatch storms after outages |
| Background processor | Separate interval + channel | Follows existing scheduled/chain/timeout patterns |
| Loop prevention | `_recurring_dispatch` metadata flag | Same pattern as `_scheduled_dispatch` |
