# Recurring Actions -- Devil's Advocate Review

This document critically examines the recurring actions architecture proposed in
`recurring-actions.md`. For each concern, the challenge is stated, its importance
is explained, and a constructive resolution is recommended.

---

## 1. Necessity -- Can Users Just Use External Cron + POST /v1/dispatch?

**Challenge**: A user can achieve the same result with a one-line crontab entry
calling `curl -X POST /v1/dispatch`. The entire recurring actions subsystem
(new core types, two new `KeyKind` variants, API endpoints, background processor
changes, admin UI, client SDK updates, documentation) is a large investment for
functionality that already has a well-understood external solution.

**Why it matters**: Feature bloat is the primary risk for infrastructure
projects. Every feature added must be maintained, tested across 6 state backends
and 5 audit backends, and documented in 4+ client SDKs. If the ROI is low, this
engineering time is better spent on features that cannot be replicated
externally, such as parallel chain steps or weighted routing.

**Recommended resolution**: The feature is justified, but only because of three
properties that external cron cannot provide:

1. **Multi-tenant management**: External cron jobs run at the OS level with no
   tenant isolation, no per-tenant pause/resume, and no visibility in the admin
   UI. Acteon's recurring actions provide tenant-scoped lifecycle management.
2. **State co-location**: The recurring action definition, execution history, and
   scheduling state all live in the same state backend. External cron requires a
   separate system for tracking what ran and when.
3. **Dynamic CRUD**: Users can create, pause, and resume recurring actions at
   runtime via API, without SSH access to a crontab.

However, the documentation should explicitly acknowledge external cron as a valid
alternative for simple use cases and position recurring actions as the choice for
teams that need runtime management, multi-tenancy, or audit trail integration.

---

## 2. Scope Creep -- Is Acteon Becoming a Job Scheduler?

**Challenge**: Acteon's core value proposition is action routing, rule
evaluation, and provider dispatch. Adding cron-based scheduling moves it toward
the territory of dedicated job schedulers (Airflow, Temporal, Celery Beat,
pg_cron). The architecture doc even says it "turns Acteon into a lightweight
scheduler." Where does Acteon's responsibility end?

**Why it matters**: Scope creep destroys focus. Once recurring actions ship,
users will request: cron expression validation UI, execution retry policies,
dependency chains between recurring actions, jitter/spread for thundering herd
prevention, calendar-aware scheduling (skip holidays), and execution history
dashboards. Each request is individually reasonable, but collectively they drag
Acteon away from its strengths.

**Recommended resolution**: Draw a clear boundary in the documentation:

- Acteon recurring actions are for **simple, periodic action dispatch**. The cron
  expression fires, a single action is dispatched through the existing pipeline.
- Acteon is NOT a workflow engine. Complex scheduling requirements (DAG
  dependencies, conditional retry, calendar awareness, exactly-once semantics)
  should use a dedicated scheduler that calls Acteon's dispatch API.
- The feature should be labeled "lightweight" or "simple" recurring actions in
  all documentation, setting user expectations.
- Explicitly list out-of-scope features in the docs: no execution retry, no
  inter-action dependencies, no calendar awareness, no backfill.

---

## 3. Cron Complexity -- Standard vs Extended, and Is Interval Simpler?

**Challenge**: The architecture specifies support for 5-field, 6-field (seconds),
and 7-field (years) cron expressions via `croner`. This is a large expression
space that is notoriously hard to validate, test, and debug. Many users who want
"every 5 minutes" or "daily at 9 AM" do not need or understand full cron syntax.

The `croner` crate is relatively new (~100 stars) compared to the `cron` crate
(~600 stars). Choosing a less-established dependency for a core feature
introduces supply chain risk.

**Why it matters**:
- Cron expressions are a common source of user errors. `"0 9 * * 1-5"` vs
  `"* 9 * * 1-5"` (every minute from 9:00 to 9:59 vs once at 9:00) is a
  classic mistake that can cause a dispatch storm.
- 6-field and 7-field cron expressions are non-standard and vary between
  implementations. Supporting them invites confusion.
- The `croner` crate's API and edge-case behavior (DST transitions, leap years,
  impossible dates) need thorough vetting before committing.

**Recommended resolution**:
- **Start with 5-field only** (minute, hour, day-of-month, month, day-of-week).
  This covers 99% of use cases and is the universally understood standard.
  6/7-field support can be added later if demanded.
- **Add an `interval_seconds` alternative** as a simpler option for common cases.
  A user who wants "every 5 minutes" should be able to write
  `"interval_seconds": 300` instead of `"cron_expr": "*/5 * * * *"`. The two
  fields would be mutually exclusive.
- **Validate aggressively**: On create/update, compute the next 5 occurrences
  and reject expressions that produce more than one occurrence per minute (this
  catches the `* 9 * * *` mistake). Return the next 3 occurrences in the API
  response so users can verify their expression is correct.
- **Vet `croner` thoroughly**: Review its DST handling, write integration tests
  for spring-forward/fall-back transitions, and pin to a specific minor version.
  If `croner` proves unreliable, the `cron` crate + manual `chrono-tz`
  conversion is a viable fallback.

---

## 4. API-Only vs Rule-Driven -- Is the Right Boundary Drawn?

**Challenge**: The architecture explicitly rejects a `RuleAction::Recurring`
variant, arguing that recurring actions are "time-driven, not condition-driven."
But this creates a conceptual split: some time-based behavior is configured
through rules (cron-based rule activation via `time.hour`), while other
time-based behavior is configured through the API (recurring actions). Users will
wonder why time-based suppression is a rule but time-based dispatch is not.

**Why it matters**: Conceptual consistency affects learnability. If a user reads
the documentation and learns that time-based behavior lives in rules, they will
look for recurring actions in the rule system. The current design forces them to
learn two different mental models for time-based features.

**Recommended resolution**: The API-only approach is correct for the reasons
stated in the architecture doc (lifecycle management, CRUD, separation of
concerns). However, the documentation must explicitly bridge the conceptual gap:

- Add a "Time-based features comparison" section to the docs that explains:
  - **Time-based rules** control *how* an action is processed (suppress during
    off-hours, reroute on weekends). They evaluate per-action at dispatch time.
  - **Recurring actions** control *when* an action is created. They are
    standalone definitions that generate actions on a schedule.
- Consider adding a convenience feature: a rule metadata key
  `_from_recurring: <id>` that lets users write rules like "if this action came
  from recurring action X, apply special routing." This bridges the two systems
  without coupling them.

---

## 5. Storage at Scale -- Impact of 100K+ Recurring Actions

**Challenge**: The architecture stores each recurring action as a separate key
in the state store and indexes it in the shared timeout BTree. With 100K
recurring actions, `get_expired_timeouts(now_ms)` returns a mixed bag of
`EventTimeout`, `PendingScheduled`, and `PendingRecurring` keys, requiring
string-based filtering. The background processor performs a full scan of expired
keys every 5 seconds, and each due recurring action triggers 6+ state store
operations (claim, load, delete pending, remove timeout, re-index, update
definition).

**Why it matters**:
- The timeout index is shared with `EventTimeout` and `PendingScheduled`. Adding
  100K `PendingRecurring` entries inflates the index, slowing all timeout-based
  features.
- The `scan_keys_by_kind(KeyKind::RecurringAction)` call in the list API is
  O(N) across all namespaces and tenants on the memory backend. On Redis it
  requires `SCAN` across the keyspace.
- Each poll cycle could trigger thousands of state store operations if many
  recurring actions fire in the same minute (e.g., `"*/5 * * * *"` on 10K
  recurring actions all fire at minute 0, 5, 10, ...).

**Recommended resolution**:
- **Add per-tenant limits**: Cap the number of recurring actions per tenant
  (e.g., 100 by default, configurable). This is both a scaling safeguard and a
  security measure (prevents a single tenant from DoS-ing the background
  processor).
- **Batch processing**: Instead of processing each due recurring action
  individually, batch the state store operations. Load all due definitions in
  one scan, process them, then batch-write the updated definitions and new
  indexes.
- **Consider a dedicated sorted set** for `PendingRecurring` (like the
  `chain_ready_at` approach) instead of sharing the timeout index. This avoids
  polluting the shared index and allows backend-specific optimizations (e.g.,
  Redis `ZRANGEBYSCORE` on a dedicated key).
- **Document scaling expectations**: State explicitly that the memory backend is
  suitable for up to ~1000 recurring actions, Redis for up to ~100K, and
  production deployments at higher scale should use Redis or Postgres.

---

## 6. Testing Difficulty -- How to Test Time-Dependent Features

**Challenge**: Cron-based features are inherently time-dependent. The background
processor polls on real wall-clock time (`Utc::now()`). Testing requires either:
- Waiting for real time to pass (slow, flaky CI).
- Injecting a clock (requires refactoring the background processor and the cron
  computation).
- Using cron expressions with very short intervals (e.g., `"* * * * *"` = every
  minute), which still requires waiting.

The existing scheduled action tests work by setting `past_due` timestamps and
letting the background processor pick them up immediately. This trick works for
scheduled actions (single-fire) but recurring actions need to verify that the
schedule advances correctly, DST transitions work, and `ends_at` is honored.

**Why it matters**: If the feature is hard to test, it will have bugs. Cron
schedule computation across DST transitions is a known source of production
incidents. Without a reliable testing strategy, regressions will ship.

**Recommended resolution**:
- **Inject a `Clock` trait** into the background processor and the cron
  computation layer. Tests provide a `MockClock` that can be advanced
  programmatically. This is a small refactor with large testing payoff.
- If a `Clock` trait is too invasive for v1, the "past-due timestamp" trick
  can still verify basic dispatch. But add explicit unit tests for the cron
  computation function in isolation (input: cron expr, timezone, reference time;
  output: next occurrence). These unit tests can cover DST, leap year, and edge
  cases without involving the background processor.
- **Test the cron library separately**: Write a dedicated test suite that
  exercises `croner` with known edge cases (spring forward: 2:30 AM in
  `US/Eastern` skips to 3:00 AM; fall back: 1:30 AM happens twice). Do not
  assume the library handles these correctly.

---

## 7. Migration Risk -- Schema Evolution for Stored Recurring Actions

**Challenge**: `RecurringAction` is serialized as JSON and stored in the state
backend. Once users create recurring actions, the struct is committed. Any future
field additions, renames, or type changes require migration logic. Unlike the
`Rule` struct (which is loaded from YAML files and can be re-parsed), recurring
actions are user-created data that persists indefinitely.

**Why it matters**: The `RecurringAction` struct has 14 fields. It is highly
likely that future iterations will need to add fields (e.g., `max_executions`,
`retry_policy`, `jitter_seconds`, `last_error`). Each addition requires backward
compatibility with existing stored definitions. Breaking changes mean data loss
or manual migration.

**Recommended resolution**:
- **Use `#[serde(default)]` on ALL optional and new fields** from day one. The
  architecture already shows this on `labels`. Apply it to `description`,
  `ends_at`, `last_executed_at`, `next_execution_at`, and `execution_count` as
  well (with sensible defaults).
- **Add a `version: u64` field** to the struct (defaulting to 1). Future
  migrations can branch on version to apply transformations when loading.
- **Store a schema version** in the state key metadata or a separate key so that
  the background processor can detect and handle legacy formats without parsing
  errors.
- **Avoid embedding the `Action` struct directly** in the stored template. The
  `RecurringActionTemplate` is a good choice because it is a smaller, more
  stable surface. If `Action` gains new required fields in the future, the
  template-to-action construction code handles the mapping, not deserialization.

---

## 8. Silent Failures -- How Does a User Know Their Recurring Action Stopped?

**Challenge**: Several scenarios can cause a recurring action to silently stop
executing without the user knowing:

- The background processor is disabled (`enable_recurring_actions = false`).
- The state backend loses the `PendingRecurring` index entry (data corruption,
  backend migration).
- The cron expression has no future occurrences (e.g., `"0 9 30 2 *"` -- Feb
  30 never occurs).
- `ends_at` is reached and the action is auto-disabled.
- The claim key expires but the pending index was cleaned up (crash during
  re-index).
- The dispatch channel is full and events are dropped.

In all these cases, the recurring action definition still exists in the state
store with `enabled: true`, but nothing happens. The user has no way to detect
this unless they manually check `last_executed_at` and compare it to the
expected schedule.

**Why it matters**: Silent failures are the worst kind of failure. A user who
sets up a daily digest email and stops receiving it after a month will lose trust
in the system. By the time they notice, they've missed 30 digests.

**Recommended resolution**:
- **Add an `expected_next_at` field** to the stored definition. If
  `last_executed_at` is more than 2x the cron interval behind
  `expected_next_at`, the recurring action is considered "stale."
- **Add a health check endpoint**: `GET /v1/recurring/health` returns recurring
  actions that are `enabled` but stale (last execution significantly overdue).
  This can be integrated into existing monitoring.
- **Emit a metric**: `recurring_stale` (gauge) tracks the count of enabled
  recurring actions whose `last_executed_at` is overdue. Alert on this metric.
- **Self-healing**: On each poll cycle, the background processor should verify
  that all `enabled` recurring actions have a corresponding `PendingRecurring`
  index entry. If not (orphaned definition), recreate the index entry. This
  handles data corruption and crash recovery edge cases.
- **Admin UI indicator**: Show a warning badge on recurring actions that haven't
  executed within their expected window.

---

## 9. Alternatives Comparison

### Option A: Simple Interval Model (every N seconds)

| Aspect | Cron-based (proposed) | Interval-based |
|--------|-----------------------|---------------|
| User complexity | Medium (cron syntax) | Low (`interval_seconds: 300`) |
| Time-of-day control | Yes (`0 9 * * *`) | No |
| DST handling | Required (complex) | N/A |
| Implementation complexity | High (cron library, timezone) | Low (simple addition) |
| Use cases covered | All periodic + calendar-aligned | Only fixed-interval |

**Verdict**: The interval model is simpler but insufficient. Daily digests at 9
AM, weekly reports on Monday, and business-hours-only schedules all require cron.
However, as noted in section 3, offering `interval_seconds` as an alternative
to `cron_expr` covers the simple cases without forcing cron complexity on users
who do not need it.

### Option B: Rule-Triggered Scheduling (RuleAction::Recurring)

| Aspect | API-only (proposed) | RuleAction variant |
|--------|---------------------|--------------------|
| Configuration | API + admin UI | YAML rule files |
| Lifecycle management | Full CRUD + pause/resume | Edit YAML, reload |
| GitOps friendly | No (runtime API) | Yes (rules in git) |
| State management | Explicit (state store) | Implicit (rule engine would need state) |
| Conceptual fit | Separate concern | Muddled (rules are per-action) |

**Verdict**: API-only is the right choice for v1. But consider supporting
rule-file-based recurring action definitions in a future iteration (e.g., a
`recurring_actions.yaml` file loaded at startup alongside rule files). This
would give GitOps teams a file-based workflow without adding a `RuleAction`
variant.

### Option C: External Scheduler Integration

| Aspect | Built-in (proposed) | External integration |
|--------|---------------------|---------------------|
| Setup complexity | Low (enable in config) | Medium (deploy scheduler + configure webhooks) |
| Multi-tenancy | Built-in | Manual (one cron job per tenant/action) |
| Observability | Integrated (metrics, admin UI) | Separate (monitor two systems) |
| Flexibility | Limited to Acteon's cron | Full scheduler capabilities |
| Maintenance burden | On Acteon team | On user/scheduler team |

**Verdict**: Built-in is the right default for most users. But document the
external integration path for teams that already have Temporal/Airflow and want
to use those for scheduling while using Acteon for dispatch.

---

## 10. Claim TTL of 60 Seconds -- Is It Right?

**Challenge**: The architecture uses a 60-second claim TTL (via `check_and_set`)
to prevent double-dispatch. This value is inherited from scheduled actions, but
recurring actions have a different profile. A recurring action dispatch involves:
building the action from a template, dispatching through the full gateway
pipeline (rules, dedup, LLM guardrails, circuit breakers, provider execution,
audit recording), and then re-indexing the next occurrence. If any provider is
slow (webhook timeout, LLM guardrail call), the dispatch can easily exceed 60
seconds.

**Why it matters**:
- If dispatch takes > 60s, the claim expires. Another instance sees the key as
  still expired (the old timestamp), claims it, and dispatches a duplicate.
- If dispatch takes < 5s (the common case), a 60s claim is wasteful -- it
  blocks crash recovery for a full minute even though the work completed in
  under a second.
- The claim TTL for scheduled actions (one-shot) has different risk: a duplicate
  scheduled action is a one-time annoyance. A duplicate recurring action creates
  a permanent double-dispatch that repeats on every tick until noticed.

**Recommended resolution**:
- **Extend the claim TTL to 120s** (2x the executor timeout default of 60s).
  This provides margin for slow providers without risking premature expiry.
- **Release the claim explicitly** after successful re-indexing, rather than
  relying on TTL expiry. The `check_and_set` pattern does not currently support
  explicit release, but a `delete(claim_key)` after processing would reclaim the
  slot immediately.
- **Add a `last_executed_at` guard** in the processor: before dispatching, check
  if `last_executed_at` is already within the current cron window. If so, skip
  (another instance already dispatched this occurrence). This is the real
  duplicate prevention; the claim is just an optimization to reduce contention.

---

## 11. Missing `max_executions` and Execution Caps

**Challenge**: The architecture has no `max_executions` field. A recurring action
with `cron_expr: "* * * * *"` (every minute) runs indefinitely. There is no
built-in mechanism to say "run this 10 times and stop" or "run this for 30 days
and then auto-delete." The `ends_at` field provides a time-based cap, but not a
count-based cap.

**Why it matters**:
- Users who want a finite number of executions (e.g., "send 5 reminder emails,
  one per day") must manually pause/delete the recurring action after the desired
  count. Forgetting to do so results in unwanted dispatches.
- Without `max_executions`, there is no way for the system to self-limit. An
  accidental `"* * * * *"` expression dispatches 525,600 actions per year with
  no automatic stop.

**Recommended resolution**:
- **Add `max_executions: Option<u64>`** to `RecurringAction`. When
  `execution_count >= max_executions`, the recurring action is auto-disabled
  (same as `ends_at` behavior). This is a low-cost addition with high safety
  value.
- **Add `min_interval_seconds: Option<u64>`** as a server-level config. This
  sets a floor on how frequently any recurring action can fire (e.g., minimum
  60 seconds). Cron expressions that resolve to a shorter interval are rejected
  at create time. This prevents accidental dispatch storms without restricting
  legitimate use cases.

---

## Summary of Recommendations

| # | Concern | Severity | Recommendation |
|---|---------|----------|---------------|
| 1 | Necessity | Low | Justified, but document external cron as an alternative |
| 2 | Scope creep | **High** | Draw explicit boundaries; label as "simple" recurring actions |
| 3 | Cron complexity | Medium | Start with 5-field only; add `interval_seconds` alternative; validate aggressively |
| 4 | API vs rules | Low | Correct choice; improve docs to bridge conceptual gap |
| 5 | Storage at scale | **High** | Per-tenant limits, consider dedicated index, batch processing |
| 6 | Testing difficulty | **High** | Inject `Clock` trait or at minimum test cron computation in isolation |
| 7 | Migration risk | Medium | `#[serde(default)]` everywhere, add `version` field |
| 8 | Silent failures | **High** | Health check endpoint, stale metric, self-healing index repair |
| 9 | Alternatives | Low | Support `interval_seconds` alongside cron; document external scheduler path |
| 10 | 60s claim TTL | Medium | Extend to 120s, release explicitly, add `last_executed_at` guard |
| 11 | No `max_executions` | Medium | Add `max_executions` field and server-level `min_interval_seconds` |

### Top 4 Must-Address Before Implementation

1. **Per-tenant limits and scaling strategy** (section 5) -- without this, a
   single tenant can degrade the system for all others.
2. **Silent failure detection** (section 8) -- without observability into missed
   executions, users will lose trust.
3. **Aggressive cron validation** (section 3) -- computing the next N
   occurrences on create/update prevents user errors from becoming runtime
   incidents.
4. **`max_executions` and `min_interval_seconds`** (section 11) -- without
   execution caps, accidental high-frequency cron expressions can cause dispatch
   storms with no automatic stop.
