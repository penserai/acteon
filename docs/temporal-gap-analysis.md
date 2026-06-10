# Gap Analysis: Acteon vs. Temporal.io (Basic Offering)

**Date:** 2026-06-10
**Status:** Implemented — all three roadmap phases shipped on this branch.
Delivered: execution event history, durable timer / wait-for-signal steps,
signal delivery with buffering, definition versioning (snapshot pinning),
visibility API with search attributes, execution reset (replay-from-step),
worker task queues with lease/heartbeat/retry/DLQ semantics, `worker` chain
steps, the checkpoint-based workflow engine with child workflows and
parent-close policies, recurring-action overlap policies (`skip` /
`cancel_other`) and windowed backfill, worker SDKs in Python / TypeScript /
Go / Java, workflow authoring SDKs in Python / TypeScript, Rust client
coverage, Executions + Workflows admin-UI pages with a history viewer and
signal delivery, docs, and a simulation example. The remaining intentional
trade-off is deterministic replay testing (the checkpoint model, §6, by
design); `continue-as-new` remains unnecessary while histories are capped.
**Scope:** What Acteon would need to credibly compete with Temporal.io's *basic* offering — the open-source, self-hosted durable-execution platform (Temporal Server + SDKs + Web UI), not Temporal Cloud's premium features (Nexus, multi-region replication, serverless workers).

---

## 1. Executive Summary

Acteon and Temporal solve adjacent but different problems today:

- **Temporal** is a *durable execution engine*: application code (workflows) runs inside customer-owned workers, and the server persists an event-sourced history so that any workflow can crash, migrate hosts, or sleep for months and resume exactly where it left off.
- **Acteon** is an *action gateway*: a policy enforcement and orchestration layer that intercepts actions, applies rules (dedup, throttle, suppress, approve, group), and dispatches them to providers, with strong audit and multi-tenancy.

The good news: Acteon already has a surprising amount of the substrate Temporal's basic offering rests on — durable state backends with CAS and distributed locks, multi-step **chains** with per-step retry/backoff/branching/parallelism, cron-based **recurring actions**, an 8-state **A2A task engine** with pause-for-human interrupts, SSE event streams, a DLQ with TLA+-verified semantics, multi-tenant namespaces, five client SDKs, and a polished UI.

The fundamental gap is **the execution model**. Temporal's core value is *workflow-as-code with durable replay*: arbitrary customer logic (loops, conditionals, local variables, `await sleep(30 days)`) survives process death because the server replays event history deterministically. Acteon's chains are *declarative server-side configurations* — powerful for routing pipelines, but they cannot express arbitrary application logic, cannot wait indefinitely on external signals, and have no replayable history.

To compete with Temporal's basic offering, Acteon needs roughly six pillars of new work, in priority order:

| # | Pillar | Gap size | Builds on |
|---|--------|----------|-----------|
| P0 | Durable event history per execution (event sourcing, not just status snapshots) | **Large** | Audit hash-chain, state CAS |
| P0 | Task queues + external worker model (workers poll, execute, report back) | **Large** | A2A `tasks:poll`, executor, DLQ |
| P0 | Workflow-as-code SDK (deterministic replay or checkpoint-based continuation) | **Very large** | Existing 5-language SDK channel |
| P1 | Signals, queries, and updates on running executions | **Medium** | Approvals, SSE, state machines |
| P1 | Durable timers at arbitrary points (`sleep(30d)` mid-execution) | **Medium** | Timeout index, chain `delay_seconds` |
| P2 | Visibility (list/filter executions by custom search attributes), versioning, child workflows | **Medium** | Audit backends, chain executions API |

A realistic strategy is **not** to clone Temporal's replay-based determinism (years of engineering, 7 SDK runtimes), but to compete on a *checkpoint/continuation-based* durable execution model layered on Acteon's existing chain engine — easier to implement, easier to explain, and differentiated by Acteon's rules/guardrails/compliance layer that Temporal entirely lacks. Section 6 details this recommendation.

---

## 2. Temporal's Basic Offering — the Bar to Clear

What a developer gets from `temporal server start-dev` plus an SDK, i.e. the feature set Acteon must answer for:

1. **Durable workflows as code** — workflows are plain functions in Go/Java/TypeScript/Python/.NET/PHP/Ruby; state lives in local variables; execution survives crashes via event-history replay.
2. **Activities** — side-effecting functions invoked from workflows, with automatic retry policies (initial interval, backoff coefficient, max attempts, non-retryable error types) and four timeout types (schedule-to-start, start-to-close, schedule-to-close, heartbeat).
3. **Task queues + workers** — the server never runs user code; workers long-poll task queues, execute workflow/activity tasks, and report results. Scaling = add workers.
4. **Durable timers** — `workflow.sleep(30 days)` consumes no resources while waiting and fires reliably.
5. **Signals / Queries / Updates** — push data into a running workflow (signal), read its state synchronously (query), or do a tracked synchronous mutation (update).
6. **Event history & replay** — complete, durable log of every state transition in an execution; the UI renders it; workers replay it to reconstruct state.
7. **Schedules** — cron-replacement with overlap policies, pause/resume, backfill.
8. **Child workflows & continue-as-new** — composition and unbounded-history mitigation.
9. **Visibility** — list/filter executions by status, type, time, and custom *search attributes*; standard + advanced (Elasticsearch-backed) visibility.
10. **Web UI + CLI** — inspect executions, history, pending activities, stack traces; terminate/cancel/signal/reset from the UI; `temporal` CLI for everything.
11. **Namespaces** — isolation units with per-namespace retention.
12. **Versioning/patching** — deploy new workflow code while old executions complete on old logic.

(2026 additions to Temporal — serverless workers, standalone activities, workflow streams — are beyond "basic" and out of scope here.)

---

## 3. Capability-by-Capability Comparison

Legend: ✅ have it · 🟡 partial / different shape · ❌ missing

| Capability | Temporal basic | Acteon today | Status |
|---|---|---|---|
| Multi-step orchestration | Workflow code | Chains: sequential, parallel groups, sub-chains, branches (`crates/core/src/chain.rs`) | 🟡 declarative only, no arbitrary logic |
| Per-step retries w/ backoff | Activity retry policies | Step-level `RetryPolicy` (fixed/linear/exponential + jitter) | ✅ comparable |
| Timeouts | 4 activity timeout types + workflow timeouts | Chain `timeout_seconds`, step delays, state-machine timeout transitions | 🟡 no schedule-to-start / heartbeat equivalents |
| Failure handling | Retry → fail workflow; saga via code | Step policies `abort`/`skip`/`dlq`; DLQ w/ manual resubmit | 🟡 no compensation/saga primitives |
| Durable timers mid-execution | `sleep()` for months | `delay_seconds` between chain steps; `Scheduled` outcome; timeout index in state store | 🟡 fixed delays only, not awaitable conditions |
| Cron / schedules | Schedules w/ overlap policies, pause, backfill | Recurring actions: cron, timezones, `max_executions`, `ends_at`, enable/disable | 🟡 missing overlap policies & backfill |
| Signals into running execution | ✅ | ❌ (approvals are the only external-input gate) | ❌ |
| Query execution state | ✅ synchronous query handlers | Chain execution status API (`GET /v1/chains/{name}/executions/{id}`) | 🟡 status snapshot, not user-defined queries |
| Event history (full log) | ✅ event-sourced, replayable | Audit trail (per-dispatch outcome records, hash-chained) + `StepAttempt` records | 🟡 audit ≠ replayable history |
| Replay / crash recovery of logic | ✅ deterministic replay | Chains resume from persisted step state via CAS (multi-replica safe) | 🟡 chains yes; arbitrary code no |
| Task queues + external workers | ✅ core model | A2A `tasks:poll` + push notifications exist for agent tasks only | 🟡 substrate exists, not generalized |
| Workers run user code | ✅ | ❌ providers run *inside* the server process | ❌ |
| Workflow-as-code SDKs | 7 languages | 5 client SDKs (Rust/Python/Node/Go/Java) — request/response only | ❌ no authoring SDK |
| Child workflows | ✅ dynamic, lifecycle-bound | Sub-chains (static references) | 🟡 |
| Continue-as-new | ✅ | ❌ (less needed without unbounded history) | ❌ |
| Versioning / patching | ✅ patch API + worker versioning | ❌ chains updated in place | ❌ |
| Visibility / search attributes | ✅ list+filter, custom indexed attributes | Audit query (tenant/outcome/provider/type/time) + analytics endpoints; ES/ClickHouse backends | 🟡 audit-shaped, not execution-shaped |
| Web UI | Execution list, history viewer, signal/cancel/reset from UI | 30+ pages incl. chain DAG visualization, execution traces, DLQ resubmit | 🟡 strong UI, missing history/replay views |
| CLI | `temporal` CLI (full surface) | `acteon` CLI + ops crate | 🟡 needs execution-management verbs |
| Local dev experience | `temporal server start-dev` (zero deps) | Memory backends, `cargo run -p acteon-server`, simulation framework | ✅ comparable |
| Namespaces / multi-tenancy | Namespaces | Namespace + tenant, grant-level authz, per-tenant rate limits & quotas | ✅ **stronger** than basic Temporal |
| AuthN/AuthZ | mTLS, pluggable authorizer (mostly Cloud) | API key, JWT, RBAC, grants, HMAC-signed approval URLs | ✅ stronger in OSS tier |
| Persistence | Cassandra/MySQL/PostgreSQL + ES visibility | State: Memory/Redis/Postgres/DynamoDB; Audit: Memory/Postgres/ClickHouse/ES/DynamoDB | ✅ comparable breadth |
| Metrics / observability | Prometheus, OTel | Prometheus, structured logging, SSE event stream | ✅ comparable |
| Human-in-the-loop | DIY via signals | First-class approvals (HMAC URLs, TTL, atomic w/ bus publish) | ✅ **stronger** |
| Rules / policy / guardrails | ❌ none | Full rules engine, CEL, WASM plugins, LLM guardrails | ✅ differentiator |
| Compliance audit | History retention only | SOC2/HIPAA modes, hash-chain tamper evidence, redaction, encryption | ✅ differentiator |
| Formal verification | Internal | TLA+ specs shipped in-repo (chain ordering, DLQ, approvals) | ✅ differentiator |

---

## 4. The Six Gaps in Detail

### Gap 1 — Durable, per-execution event history (P0, large)

**Temporal:** every execution has an append-only event history (`WorkflowExecutionStarted`, `ActivityTaskScheduled`, `TimerFired`, …) that is the source of truth: the UI renders it, workers replay it, `reset` rewinds it.

**Acteon today:** `ChainExecution` persists *current* status + `StepAttempt` records (good), and the audit trail records *dispatch outcomes* (good, even tamper-evident), but neither is a complete, ordered, replayable event log of an execution. There is no way to reconstruct "what the execution knew at step 3" or reset an execution to a prior event.

**What to build:**
- A new `ExecutionHistory` abstraction in `acteon-core`: append-only events keyed by `execution_id` with monotonic `event_id`, stored via the existing `StateStore`/`AuditStore` traits (the hash-chain machinery in `HashChainAuditStore` is directly reusable for ordering + integrity).
- Event types covering the chain lifecycle first (step scheduled/started/completed/failed/retried, timer started/fired, signal received, execution completed/failed/canceled).
- `GET /v1/executions/{id}/history` + UI history viewer (the chain DAG page is the natural host).
- `reset`/`replay-from-event` comes later; the log itself is the prerequisite for everything else (signals, queries, worker recovery, debugging).

### Gap 2 — Generalized task queues + external workers (P0, large)

**Temporal:** the server *never executes user code*. Workers long-poll named task queues; this is what makes Temporal safe to multi-tenant and infinitely scalable on the execution side.

**Acteon today:** providers execute *inside* the server process (`crates/executor`). This is fine for notification fan-out but is the single biggest architectural divergence: a competitor offering must let customers run their own code against the orchestrator. Crucially, **the substrate already exists**: the A2A engine has `POST /a2a/{ns}/{tenant}/v1/tasks:poll`, task leasing semantics (Working state, stale-task reaper as lease expiry), push notifications, and CAS-guarded transitions.

**What to build:**
- Promote the A2A poll/lease/complete loop into a first-class, protocol-neutral **task queue** primitive: named queues per namespace/tenant, long-poll endpoint, lease TTL + heartbeat (extend lease), `complete`/`fail` callbacks feeding the chain engine.
- A new provider type `worker_queue` so a chain step can target "whatever worker polls queue X" instead of a built-in integration. This makes every existing chain feature (retries, branches, parallelism, DLQ) immediately available to customer code.
- Worker-side: sticky routing and concurrency limits can come later; lease+heartbeat is the MVP.
- Reuse: stale-task reaper → lease expiry; DLQ → activity max-attempts exhaustion; circuit breakers → per-queue backpressure.

### Gap 3 — Workflow-as-code SDK (P0, very large — but see §6 for the shortcut)

**Temporal:** the moat. Deterministic workflow runtimes in 7 languages, replay testing, sandboxing, patch/versioning APIs. Replicating this faithfully is a multi-year effort.

**Acteon today:** five SDKs exist but are thin HTTP clients. Chains are authored as YAML/TOML/JSON-over-API.

**What to build (two options):**
- **Option A — full replay determinism (Temporal-style):** deterministic event loop per language, history replay, sandbox rules ("no random, no wall clock, no I/O in workflow code"). Highest fidelity, highest cost, and Temporal's 7-year head start applies.
- **Option B — checkpoint/continuation model (recommended, §6):** workflow code runs on customer workers as a series of *steps between awaits*; at every `await` (activity result, timer, signal) the SDK persists a named checkpoint + serialized state to Acteon and the function exits; on resume, the SDK re-enters at the checkpoint with restored state. No determinism requirement, no replay, no sandbox. This is the model of Restate/Inngest/Hatchet/Azure Durable Functions (entity mode) — all credible Temporal competitors that shipped it with small teams.
- Either way, ship a *workflow authoring* layer in the existing SDK channel, starting with TypeScript + Python (largest Temporal-adjacent audiences), Rust third.

### Gap 4 — Signals, queries, updates (P1, medium)

**Temporal:** `signal` (async push into execution), `query` (sync read), `update` (sync tracked mutation).

**Acteon today:** the only external inputs to a running chain are approvals (which are exactly a hard-coded signal: "human said yes/no") and event-state transitions (`PUT /v1/events/{fingerprint}/transition`). The bus has conversations/tool-calls but they aren't wired to chain executions.

**What to build:**
- `POST /v1/executions/{id}/signal/{name}` — appends a `SignalReceived` event to history; a new chain step type `wait_for_signal` (with optional timeout → branch) blocks until it arrives. Generalize approvals to be a signal with a signed-URL transport — one mechanism, two front doors.
- `GET /v1/executions/{id}/state` — for declarative chains this is derived automatically (current step, accumulated `{{steps.*}}` context); for SDK workflows, the worker registers query handlers served via the task-queue channel.
- Updates can wait; signal+query covers the basic offering.

### Gap 5 — Durable timers as awaitable primitives (P1, medium)

**Temporal:** `sleep(30 days)`, timers cancellable, visible in history.

**Acteon today:** the pieces exist — `index_timeout`/`get_expired_timeouts` (O(log N) timer wheel in every state backend), chain `delay_seconds`, `Scheduled {scheduled_for}` outcome, state-machine `after_seconds` transitions — but they are scattered and not exposed as a single awaitable primitive.

**What to build:**
- A `timer` chain step type (`wait_until` / `wait_for`) recorded as `TimerStarted`/`TimerFired` history events, cancellable, surfaced in the UI.
- For SDK workflows: `await ctx.sleep(...)` persists a checkpoint + timeout-index entry; the background processor (which already drives chain advancement and group flushing) wakes the execution. The `BackgroundProcessor` needs no new architecture, just a new event type alongside `ChainAdvanceEvent`.
- Long-duration correctness: timers must survive backend failover — already true for Postgres/DynamoDB state backends; document Redis caveats (same story as locks).

### Gap 6 — Visibility, versioning, child workflows (P2, medium)

- **Visibility:** add an *executions* index (distinct from audit): list/filter by chain/workflow type, status, start/close time, plus user-defined **search attributes** (typed key-values attached at start or upserted mid-run). The Elasticsearch and ClickHouse audit backends are exactly the right engines; this is mostly schema + API + UI work (`GET /v1/executions?query=...`). Reuse the analytics endpoints' query plumbing.
- **Versioning:** version chain definitions (`name@v3`); in-flight executions pin the version they started with; new starts use latest. This is far easier in a declarative model than Temporal's patch API — a genuine advantage of chains. SDK workflows under the checkpoint model need only checkpoint-compatibility rules, not replay-compatibility.
- **Child workflows:** allow a chain step / SDK call to *start* another execution dynamically (today sub-chains are static references), with parent-close policies (abandon/cancel/terminate). The A2A reference-graph cycle detection already solves the hard correctness problem here.
- **Continue-as-new:** only needed once histories grow unbounded; defer.

---

## 5. Where Acteon Is Already Ahead (Keep and Market These)

A competitor pitch is not just gap-closing. Against Temporal's basic offering, Acteon should lead with what Temporal cannot do:

1. **Policy layer on every step.** Temporal executes whatever the workflow says. Acteon can dedup, throttle, suppress, silence, quota-enforce, LLM-guardrail, and require human approval on *any* step — declaratively, hot-reloadable, per tenant. "Durable execution with a policy firewall" is a category Temporal doesn't occupy.
2. **Compliance-grade audit.** Hash-chained, tamper-evident, redacting/encrypting audit with SOC2/HIPAA modes vs. Temporal's plain history retention.
3. **First-class human-in-the-loop.** Signed approval URLs with TTLs vs. "build it yourself with signals."
4. **Real multi-tenancy in OSS.** Per-tenant quotas, rate limits, grants, connection caps — Temporal's OSS namespaces are isolation-only; serious tenancy is a Cloud feature.
5. **AI-agent surface.** A2A protocol, agentic bus, swarm orchestration, MCP server — Temporal is only now bolting agent SDKs on. Durable execution *for agent fleets, with guardrails* is the most defensible wedge.
6. **Built-in integrations.** 20+ providers out of the box; with Temporal, every Slack message is an activity you write yourself.
7. **Single static binary in Rust.** Operationally simpler than Temporal's four services + Cassandra/ES footprint; memory-backend dev mode already matches `start-dev`.

---

## 6. Recommended Strategy & Phasing

**Do not clone Temporal's replay model.** Compete with a **checkpoint/continuation durable-execution engine** built on the chain substrate, and differentiate on policy/compliance/agents.

### Phase 1 — "Durable chains" (foundation, no SDK yet)
- Execution history log (Gap 1) for chain executions; history viewer in UI.
- Timer step type + `wait_for_signal` step type with signal API (Gaps 4–5, declarative side).
- Executions visibility API + search attributes (Gap 6a).
- Chain definition versioning (Gap 6b).
- *Outcome:* chains become honest long-running workflows — pause for a signal for a month, sleep 30 days, full history, filterable. Competitive with Temporal for declarative/low-code use cases, which Temporal serves poorly.

### Phase 2 — "Bring your own code" (worker model)
- Generalize A2A poll/lease into named task queues; `worker_queue` provider type (Gap 2).
- Worker shims in the existing TypeScript/Python/Go SDKs: register handler, poll, heartbeat, complete/fail.
- DLQ + retries + circuit breakers automatically apply to customer activities.
- *Outcome:* parity with Temporal's activity model; customer code runs outside the server.

### Phase 3 — "Workflow SDK" (checkpoint model)
- `@acteon/workflows` (TS) and `acteon-workflows` (Python): `ctx.step()`, `ctx.sleep()`, `ctx.waitForSignal()`, `ctx.startChild()` with serialized-state checkpointing to the history log (Gap 3, Option B).
- Replay-free recovery: resume at last checkpoint on any worker.
- Child executions + parent-close policies (Gap 6c).
- *Outcome:* "workflows as code" headline feature; the basic-offering checklist in §2 is fully answerable except deterministic replay/reset, which the checkpoint model intentionally trades away.

### Effort sanity check
Phase 1 is mostly recombination of existing machinery (state CAS, timeout index, hash-chain, background processor, chain engine) — quarters, not years. Phase 2 leans on the A2A engine — comparable scope to what A2A itself took. Phase 3 is the genuinely new engineering; scoping it to two languages and the checkpoint model keeps it tractable. Precedent: Inngest, Restate, and Hatchet each reached a credible Temporal-basic alternative with this model and small teams.

### Risks
- **Determinism purists:** checkpoint model can't `reset` to an arbitrary event or unit-test via replay. Mitigation: position history `reset` for declarative chains only; document the trade-off honestly.
- **Two execution models** (declarative chains vs. SDK workflows) risk product confusion. Mitigation: SDK workflows *are* chains under the hood (dynamic chains whose steps are checkpoints) — one engine, one history format, one UI.
- **In-process providers vs. worker model** must not fork the executor; the `worker_queue` provider keeps both paths inside the existing dispatch pipeline so rules/audit/quotas apply uniformly.

---

## 7. Basic-Offering Scorecard (Today → After Phase 3)

| Temporal basic feature | Today | After P1 | After P2 | After P3 |
|---|---|---|---|---|
| Durable multi-step execution | 🟡 | ✅ | ✅ | ✅ |
| Activities w/ retry policies | 🟡 in-process only | 🟡 | ✅ | ✅ |
| Task queues / workers | ❌ | ❌ | ✅ | ✅ |
| Workflows as code | ❌ | ❌ | ❌ | ✅ (checkpoint) |
| Durable timers | 🟡 | ✅ | ✅ | ✅ |
| Signals | ❌ | ✅ | ✅ | ✅ |
| Queries | 🟡 | ✅ (derived) | ✅ | ✅ (handlers) |
| Event history | 🟡 | ✅ | ✅ | ✅ |
| Replay/reset | ❌ | 🟡 chains only | 🟡 | 🟡 by design |
| Schedules | 🟡 | ✅ (+overlap/backfill) | ✅ | ✅ |
| Child workflows | 🟡 static | 🟡 | 🟡 | ✅ |
| Versioning | ❌ | ✅ chains | ✅ | ✅ |
| Visibility + search attributes | 🟡 | ✅ | ✅ | ✅ |
| Web UI for executions | 🟡 | ✅ | ✅ | ✅ |
| Namespaces/multi-tenancy | ✅ | ✅ | ✅ | ✅ |
| Local dev experience | ✅ | ✅ | ✅ | ✅ |

---

## Appendix A — Key Acteon Source References

| Capability | Location |
|---|---|
| Chains (steps, retries, branches, parallel) | `crates/core/src/chain.rs`, `crates/server/src/api/chains.rs`, `docs/book/features/chains.md` |
| Chain advancement / background processing | `crates/gateway/src/` (`BackgroundProcessor`, `ChainAdvanceEvent`), `get_ready_chains()` in state trait |
| Recurring actions (cron) | `crates/core/src/recurring.rs`, `crates/server/src/api/recurring.rs` |
| State store trait (CAS, timeout index, locks) | `crates/state/state/src/store.rs`, `crates/state/state/src/lock.rs` |
| Audit + hash chain | `crates/audit/audit/src/store.rs` (decorators: redacting, encrypting, hash-chain, compliance) |
| A2A task engine (poll/lease/transitions) | `crates/core/src/bus_task.rs`, `crates/server/src/api/a2a.rs` |
| Approvals (pause-for-human) | `crates/core/src/bus_approval.rs`, `crates/server/src/api/approvals.rs` |
| DLQ | `crates/executor/src/dlq.rs` |
| Executor (retries, concurrency) | `crates/executor/` |
| Client SDKs | `crates/client/`, `clients/{python,nodejs,go,java}/` |
| TLA+ specs | `specs/` |

## Appendix B — Sources on Temporal's Offering

- Temporal product overview: https://temporal.io/ and https://temporal.io/product
- Workflow concepts: https://docs.temporal.io/workflows and https://docs.temporal.io/workflow-execution
- Understanding Temporal: https://docs.temporal.io/evaluate/understanding-temporal
- Durable execution definition: https://temporal.io/blog/what-is-durable-execution
