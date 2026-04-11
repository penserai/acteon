# Alertmanager Feature Parity — Master Plan

**Status**: In Progress (Phase 1)
**Author**: Acteon team
**Date**: 2026-04-10

## 1. Objective

Make Acteon a functional replacement for Prometheus Alertmanager so that
customers running Alertmanager today can switch to Acteon without losing
capability. Capability parity, not wire-protocol compatibility.

### 1.1 What "feature parity" means here

- Every Alertmanager capability that ops teams depend on exists in Acteon, using Acteon's idioms (Actions, rules, providers, grants).
- Customers migrate by re-authoring their `alertmanager.yml` into Acteon rules and providers, not by pointing Prometheus at a compat endpoint.
- Customers who need migration assistance get an opinionated importer tool as a separate, later initiative — not in scope for this plan.

### 1.2 Explicit non-goals

- **No `POST /api/v2/alerts` compat endpoint.** Acteon ingests via its native dispatch API.
- **No `alertmanager.yml` parser.** If demand emerges, ship it as a standalone CLI (`acteon import alertmanager-config`) in a later initiative.
- **No amtool / promtool wire compatibility.**
- **No first-class `Alert` struct in `acteon-core`.** Alerts flow through the generic `Action` type with a documented payload convention. Aesthetic polish we'll reconsider only if customer feedback demands it.
- **No gossip-based HA cluster.** Acteon's distributed state backends already provide equivalent coordination; this plan documents the guarantee rather than re-implementing Alertmanager's gossip layer.

---

## 2. Feature gap audit (2026-04-10)

A fresh audit against Alertmanager v0.27 features found the following gaps.

| # | Alertmanager capability | Acteon status | Severity |
|---|---|---|---|
| 1 | **Silences** (time-bounded label-pattern mutes) | **Missing** — no `Silence` type, no CRUD, no dispatch-path enforcement | **Critical** |
| 2 | `group_wait` / `group_interval` / `repeat_interval` | **Shipped in Phase 2.** `group_wait` was already implemented; `group_interval` is now honored on persistent groups; `repeat_interval` is a new optional field that makes the group persistent and re-fire periodically. | ~~Medium~~ Done |
| 3 | Per-receiver rate limits | **Shipped in Phase 3.** Generic tenant/namespace quotas now stack with optional per-provider scoped policies; strictest outcome wins; each scope has its own counter bucket. | ~~Medium~~ Done |
| 4 | OpsGenie receiver | Missing | Medium |
| 5 | VictorOps receiver | Missing | Medium |
| 6 | Pushover receiver | Missing | Low |
| 7 | WeChat receiver | Missing | Low |
| 8 | Telegram receiver | Missing | Low |
| 9 | Alert-centric admin UI (active alerts grouped by labels) | Missing — UI is action-centric and event-centric | Low |
| 10 | First-class `Alert` primitive | Missing | **Skip** — handle via generic `Action` + convention |

### 2.1 Capabilities already at parity (no work needed)

These were audited and found complete:

- **Alert ingestion**: `POST /v1/dispatch` accepts alert-shaped payloads today
- **Dedup**: `Action.dedup_key` + atomic CAS against distributed state store
- **Label-based routing**: rules with `Reroute` action
- **Inhibition**: rules with `Suppress` action
- **Grouping**: `EventGroup` + `GroupManager` (first-wait only — see gap #2)
- **Templates**: payload templates with MiniJinja
- **HA clustering**: distributed state backend coordination provides dedup-across-replicas guarantees
- **Email, Slack, PagerDuty, Discord, Teams, Twilio, Webhook receivers**: all present
- **Audit trail**: full provenance of every dispatched action
- **Tenant-scoped authorization**: grants with `providers`/hierarchical tenant matching (shipped in #82)

---

## 3. Phasing

Each phase is an independent, shippable PR. Phases 2–5 can be reordered or parallelized based on customer demand. Phase 1 is critical-path — without it, Acteon cannot claim Alertmanager parity.

### Phase 1 — Silences (this PR)
**Gap**: #1
**Scope**: ~2000 LOC
**Critical path**: yes

Silences are the biggest functional gap and the single feature that blocks the parity claim. Ops teams use silences during maintenance windows to temporarily mute alerts without modifying rules. Without this, Acteon is strictly less capable than Alertmanager for any on-call team.

### Phase 2 — Complete event grouping intervals ✅ Shipped
**Gap**: #2
**Status**: Shipped in PR #84 + post-review follow-up.
**Scope as built**: ~1200 LOC across core types, gateway group manager, background worker, rule IR/YAML, HA plumbing, and tests.

### Phase 2.1 — Known follow-ups
- **Configurable `max_group_size` drop policy**: Phase 2 hard-codes drop-oldest (FIFO). A future revision should expose `drop_policy: {oldest, newest, middle}` per rule so operators whose "first events are most important" (typical root-cause investigation) can opt into keep-first. Documented in `docs/book/features/event-grouping.md`.
- **Pending-groups index usage**: the `KeyKind::PendingGroups` secondary index is still only written, never read by the flush discovery path. Cleanup is a small polish item.

`group_interval_seconds` is now honored on persistent groups (those with `repeat_interval_seconds` set). `repeat_interval_seconds` is a new optional field on the Group rule action; when present, the group survives its first flush, re-batches new events using `group_interval_seconds`, and re-fires on the repeat interval with no new events. Ephemeral groups (no `repeat_interval_seconds`) preserve pre-Phase-2 behavior exactly for backward compatibility.

Implementation notes:
- `EventGroup` gains `last_notified_at`, `group_wait_seconds`, `group_interval_seconds`, `repeat_interval_seconds`, `max_group_size`. All new fields use serde defaults so old state-store records deserialize cleanly.
- `GroupManager::add_to_group` handles the `Notified → Pending` transition on new events for persistent groups, recomputing `notify_at` as `max(last_notified_at + group_interval, now)`.
- `GroupManager::flush_group` schedules the next `notify_at` at `now + repeat_interval` for persistent groups.
- `get_ready_groups` picks up persistent `Notified` groups whose `notify_at` has arrived, but ignores ephemeral groups stuck in `Notified` (fail-safe against the flush worker being interrupted between `flush` and `remove`).
- `max_group_size` is now actually enforced — `EventGroup::add_event` drops the oldest event when at capacity.
- The background flush worker only deletes ephemeral groups; persistent groups stay alive in memory.

### Phase 3 — Per-provider rate limits ✅ Shipped
**Gap**: #3
**Status**: Shipped in PR #85.
**Scope as built**: ~1000 LOC across core types, gateway multi-policy enforcement, server CRUD, CLI flag, polyglot SDKs, and tests.

`QuotaPolicy` gained an optional `provider: Option<String>` field.
Multiple policies may coexist for the same `(namespace, tenant)`
pair as long as their provider scopes differ (one generic catch-all
plus any number of per-provider policies). At dispatch time, every
policy whose scope matches the outgoing provider is evaluated, each
maintains its own counter bucket
(`{ns}:{tenant}:{provider_or_*}:{window}:{idx}`), and the strictest
applicable outcome wins (Block > Degrade > Warn > Notify). When any
policy blocks a dispatch, every counter touched during that call is
rolled back so the blocked request never consumes sibling budgets.

Implementation notes:
- Backward compatibility: missing `provider` field deserializes as `None` (generic) via `#[serde(default)]`; the old `idx:{ns}:{tenant}` index key value format (bare UUID) is accepted on read alongside the new JSON-array format.
- Gateway cache changes: `quota_policies` is now `HashMap<String, CachedPolicy>` where `CachedPolicy` is a bucket of policies for a `(ns, tenant)`; cold-path loader fetches the full bucket in a single index lookup + N policy reads.
- Server CRUD: duplicate detection now keys on `(namespace, tenant, provider)` rather than just `(namespace, tenant)`; the list endpoint accepts a `provider` query param (`generic` matches catch-alls, anything else is an exact match).
- CLI: `acteon quotas list --provider slack` filters the listing; `acteon quotas create` accepts `provider` in the JSON payload.
- Polyglot SDKs: Rust, Python, Node.js, Go, and Java client models all gained the `provider` field and the provider filter on `list_quotas`.

### Phase 4 — Missing receivers
**Gaps**: #4–#8
**Scope**: ~300–500 LOC per provider
**Follow-up**: one provider per PR, batched where sensible (e.g., OpsGenie + VictorOps in one PR; Pushover + WeChat + Telegram in another).

### Phase 5 — Alert-centric admin UI
**Gap**: #9
**Scope**: ~600 LOC, depends on Phase 1
**Follow-up**: new `ui/src/pages/Silences.tsx` showing silences; new `ui/src/pages/Alerts.tsx` showing active alerts grouped by labels with inline silence/ack controls.

### Phase 6 — Polyglot SDK silences support
**Depends on**: Phase 1
**Scope**: ~400 LOC
**Follow-up**: thin HTTP wrappers in Python, Node.js, Go, Java SDKs for silences CRUD, following the pattern established by quotas and retention.

---

## 4. Phase 1 detailed design — Silences

### 4.1 Data model

```rust
pub struct Silence {
    pub id: String,                    // UUID v7
    pub namespace: String,
    pub tenant: String,
    pub matchers: Vec<SilenceMatcher>, // AND semantics — all must match
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_by: String,            // from CallerIdentity
    pub comment: String,
    pub created_at: DateTime<Utc>,
}

pub struct SilenceMatcher {
    pub name: String,  // label key (against action.metadata.labels)
    pub value: String, // literal or regex pattern
    pub op: MatchOp,
}

pub enum MatchOp { Equal, NotEqual, Regex, NotRegex }
```

A silence matches an action iff **every** matcher in the silence matches that
action (AND semantics). Matchers are evaluated against the action's
`metadata.labels` map only — the payload is out of scope.

### 4.2 Regex complexity cap

Alertmanager allows regex matchers, which are useful (`severity=~"warning|critical"`) but are also a ReDoS vector if user input is unbounded.

Acteon caps regex matchers at creation time:

- Pattern length ≤ **256 characters**
- Compiled using `regex::RegexBuilder::size_limit(65_536)` and `dfa_size_limit(65_536)`, rejecting any regex whose compiled DFA exceeds these bounds
- Backtracking patterns (lookaround, backreferences) are inherently rejected because the `regex` crate does not support them

Invalid regexes are rejected with `400 Bad Request` at silence creation. No way to create a silence that could ReDoS the dispatch path.

### 4.2b Cache layout and hierarchical enforcement

The in-memory cache is keyed by namespace alone — `HashMap<Namespace, Vec<CachedSilence>>`.
Each silence's tenant is checked at dispatch time against the action's
tenant using a dot-strict hierarchical match: a silence on tenant
`acme` covers dispatches to `acme`, `acme.us-east`, and
`acme.us-east.prod`, but not `acme-corp` (no dot separator) or
`acme.eu-west.prod` → `acme.eu-west` (no inheritance upward).

The cache is NOT keyed by `(namespace, tenant)` because that would
prevent hierarchical inheritance — an exact hashmap lookup on
`(acme.us-east, ...)` would miss a silence created on parent `acme`.

### 4.2c Distributed synchronization (HA)

Silences are eventually consistent across gateway instances via the
background processor's `enable_silence_sync` tick (default: 10 seconds).
Each tick calls `Gateway::sync_silences_from_store`, which rebuilds
the cache from `KeyKind::Silence` entries. This means:

- An operator creating a silence via instance A will see it take
  effect immediately on instance A (CRUD updates the local cache
  synchronously).
- Other instances will pick up the new silence within the sync
  interval (default 10s).
- Deletes propagate the same way: the soft-expired silence still
  exists in the state store but peer instances will observe
  `ends_at < now` after their next sync.

For production deployments, operators should leave `enable_silence_sync`
at its default `true`. Disabling it is only appropriate for single-instance
deployments where split-brain is not a concern.

### 4.3 Where silences are evaluated in the dispatch pipeline

**After rule evaluation, before provider dispatch.**

```
dispatch()
  ├─ auth grant check        ← enforced (existing)
  ├─ quota check              ← enforced (existing)
  ├─ template rendering       ← existing
  ├─ rule evaluation          ← existing (may modify payload)
  ├─ silence check            ← NEW
  │    └─ if matched: emit ActionOutcome::Silenced { silence_id }
  │                   write audit record with matched rule + silence
  │                   return without calling provider
  └─ provider dispatch        ← skipped if silenced
```

**Rationale**: running the silence check after rules preserves the rule verdict in the audit record ("this would have matched rule X but was silenced by Y"). This matches Alertmanager's semantics, where silences affect notification delivery, not the evaluation itself. Operators get full forensic context when debugging silenced alerts.

### 4.4 `ActionOutcome::Silenced` variant

New enum variant:

```rust
ActionOutcome::Silenced {
    silence_id: String,
    matched_rule: Option<String>,  // from the preceding rule eval
}
```

Written to the audit trail with `outcome = "silenced"`. Surfaceable via the audit query, rule coverage report, and analytics.

### 4.5 Tenant scoping

Silences are stored under `(namespace, tenant)`. All CRUD operations enforce the caller's grants via the existing `is_authorized` path:

- `POST /v1/silences` — caller must have a grant covering `(namespace, tenant, *, *)`
- `GET /v1/silences?namespace=...&tenant=...` — filter is auto-injected for single-tenant callers
- `DELETE /v1/silences/{id}` — caller must have a grant covering the silenced `(namespace, tenant)`

Hierarchical tenant matching applies: a caller scoped to `acme` can create silences in `acme.us-east`.

### 4.6 Permission model

New `Permission::SilencesManage`. Held by `admin` and `operator` roles but NOT by `viewer`. This is distinct from `RulesManage` — on-call operators need to silence without being able to modify rules. See the [API Key Scoping](book/features/api-key-scoping.md) docs for the role matrix.

### 4.7 Storage

Silences live in the state store under key prefix `silence:{namespace}:{tenant}:{id}`. Loaded into an in-memory cache on startup. CRUD operations update both the cache and the state store atomically. The cache is consulted on every dispatch for O(1) lookup; scanning all active silences is acceptable because the cache is small (ops teams typically have <100 active silences).

Not persisted in the state store but stored in the cache: compiled regex matchers. Rebuilt on cache reload.

### 4.8 Expiry semantics (soft-delete)

Silences are never hard-deleted via the API. `DELETE /v1/silences/{id}`
sets `ends_at = now` and persists the updated record. Rationale:

- **Audit-reference integrity**: `ActionOutcome::Silenced` records a
  `silence_id` in the audit trail. An operator later investigating "why
  was this alert silenced?" must be able to resolve the ID back to the
  original matchers + comment + creator. A hard delete leaves a dangling
  reference.
- **Dispatch-path correctness**: once `ends_at = now`, the cached
  entry's `is_active_at(now)` check returns false, so the next
  dispatch is not silenced. The cache entry remains in memory (its
  pre-compiled regex is still valid) and is listed via
  `GET /v1/silences?include_expired=true` until a future reaper
  removes the tombstone from the state store.

**Background reaper is deferred to Phase 1.5**. In Phase 1, the state
store accumulates soft-expired silences indefinitely. For typical ops
usage (<100 silences per tenant per month) this is negligible; a reaper
will be added in a short follow-up PR that tombstones entries older
than 7 days.

### 4.9 API

| Method | Path | Description |
|---|---|---|
| `POST` | `/v1/silences` | Create a silence. Requires `SilencesManage`. |
| `GET` | `/v1/silences` | List active silences. Tenant filter auto-injected for scoped callers. Optional `include_expired=true` shows expired silences up to 7 days old. |
| `GET` | `/v1/silences/{id}` | Get a silence by ID. |
| `PUT` | `/v1/silences/{id}` | Extend `ends_at` or edit `comment`. Matchers are immutable. |
| `DELETE` | `/v1/silences/{id}` | Expire a silence immediately. Does not delete. |

### 4.10 CLI

```bash
# Create a 2-hour silence during maintenance
acteon silences create \
  --namespace prod --tenant acme \
  --matcher severity=warning \
  --matcher team=platform \
  --duration 2h \
  --comment "deploying canary"

# List active silences
acteon silences list --namespace prod

# Get a specific silence
acteon silences get <id>

# Expire a silence early
acteon silences expire <id>
```

### 4.11 What's in this PR vs. follow-ups

**In this PR**:
- Master plan (this document)
- Core types (`Silence`, `SilenceMatcher`, `MatchOp`, `ActionOutcome::Silenced`)
- Core state key (`KeyKind::Silence`)
- Gateway silence store + background reaper + dispatch-path enforcement
- Server CRUD handlers + `SilencesManage` permission + route registration + OpenAPI schemas
- Rust client (`acteon-client`) silences module + ops wrapper
- CLI `acteon silences` subcommand
- Unit tests (matchers, cache)
- Integration tests (CRUD, scope enforcement, dispatch interception, permission checks)
- Feature docs page (`docs/book/features/silences.md`) linked from `api-key-scoping.md`

**Deferred to follow-up PRs**:
- Polyglot SDKs (Python, Node.js, Go, Java) — Phase 6
- Admin UI pages for silences and alerts — Phase 5
- Simulation example — bundled with Phase 6

---

## 5. Open questions

1. **Should expired silences auto-delete after 7 days, or stay forever?** Current design: 7-day tombstone then delete. Rationale: audit trail already captures every silenced dispatch; the silence itself is metadata. Keeping forever bloats the silence list. Revisit if compliance customers need longer retention.

2. **Should `PUT /v1/silences/{id}` allow extending `starts_at` backward for retroactive silences?** Current design: no. A silence only affects future dispatches. Revisit if retroactive audit adjustments become a customer ask (they shouldn't — that's tampering).

3. **How should the CLI format durations?** Current design: `--duration 2h` / `--duration 30m` / `--duration 7d`. Could also accept RFC 3339 `--ends-at 2026-04-11T00:00:00Z`. Ship both.

4. **Should silences support namespace/tenant wildcards?** Current design: no. A silence is always scoped to one `(namespace, tenant)`. Wildcards would make cross-tenant silencing possible which is dangerous for multi-tenant deployments. Revisit if a single-tenant customer complains.

---

## 6. Success criteria for Phase 1

- [ ] Unit tests: matcher semantics (Equal, NotEqual, Regex, NotRegex, multi-matcher AND)
- [ ] Unit tests: regex complexity cap rejects oversized patterns
- [ ] Gateway test: dispatch intercepts a matching action and emits `ActionOutcome::Silenced`
- [ ] Gateway test: non-matching action is not silenced
- [ ] Gateway test: expired silence does not intercept
- [ ] Server test: CRUD happy path
- [ ] Server test: tenant-scoped caller cannot create silence in another tenant
- [ ] Server test: `SilencesManage` permission enforcement
- [ ] Server test: hierarchical tenant — grant on `acme` covers silence on `acme.us-east`
- [ ] Server test: `GET /v1/silences?include_expired=true` returns expired silences
- [ ] Regex complexity cap: patterns >256 chars rejected
- [ ] `acteon silences create/list/get/expire` CLI commands work end-to-end
- [ ] `docs/book/features/silences.md` rendered in mkdocs nav
- [ ] `cargo clippy --workspace --no-deps -- -D warnings` passes
- [ ] `cargo test --workspace --lib --bins --tests` passes
- [ ] `cd ui && npm run lint && npm run build` passes

---

## 7. Phase 1 PR structure

Single PR titled `feat: silences — Phase 1 of Alertmanager parity`, containing all items listed in section 4.11. Follow-up PRs track phases 2–6, each referencing this document.
