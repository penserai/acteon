# Agentic Bus — Phase 9

> **Scope**: closeout. A consolidated user guide replacing nine
> phase docs as the entry point, a migration guide for moving
> existing dispatch + chain pipelines onto the bus, a runnable
> multi-agent demo combining every primitive end-to-end, and a
> Criterion bench measuring typed-envelope overhead. No new wire
> surface. See the [master plan](../concepts/bus-master-plan.md).

Phases 1–8 built the bus, the UI, and the polyglot SDKs. Phase 9
is the wrap-up: docs that read as one coherent story rather than
a changelog, and the artifacts (demo + benchmarks) that let
operators evaluate the bus without re-deriving everything from
the source.

## What ships in Phase 9

| Surface | Shape |
|---|---|
| User guide | [`concepts/agentic-bus.md`](../concepts/agentic-bus.md) — single page covering the model, the seven primitives, the layered topic design, the full REST surface, and "when to use what" guidance |
| Migration guide | [`guides/agentic-bus-migration.md`](../guides/agentic-bus-migration.md) — concrete before/after for the four typical refactors (fan-out dispatch → topic + subs; tool-using loop → conversations; streaming output → stream envelopes; high-risk action → HITL approval) |
| Multi-agent demo | `crates/simulation/examples/multi_agent_demo.rs` — three agents (planner, calendar, summarizer) collaborating on one conversation, exercising every Phase 1–6c primitive end-to-end |
| Benchmarks | `crates/simulation/benches/bus_overhead.rs` + [`reference/bus-benchmarks.md`](../reference/bus-benchmarks.md) — Criterion suite measuring typed-envelope overhead vs raw publish, methodology + reference numbers |
| Master plan refresh | All phases marked shipped; the bus master plan stops being a roadmap and becomes a reference for what was built |

## The user guide

The new [`concepts/agentic-bus.md`](../concepts/agentic-bus.md)
is the page a new operator should read first. It covers:

- **Why a bus.** The dispatch model is imperative; agent fleets
  need conversational. Where the line falls.
- **The seven primitives.** Topics, schemas, subscriptions,
  agents, conversations, tool envelopes, stream envelopes,
  approvals — what each owns and when to reach for it.
- **Layered design.** Why tool-calls / streams / approvals all
  ride one events topic, not parallel pipelines.
- **The full REST surface** in one place — every endpoint
  Phases 1–6c shipped, grouped by phase.
- **A complete tour** that points at the multi-agent demo so a
  new operator can see every primitive working in one runnable
  artifact.
- **Trust model summary** — the recurring themes (single source
  of truth, operator-asserted correlation tokens, participant
  ACL on writes, V1 non-atomic HITL parking) so an operator
  doesn't need to read all nine phase docs to understand the
  guarantees.

The phase-by-phase docs (`bus-phase-1.md` … `bus-phase-8.md`)
remain the historical record for what each release introduced.

## The migration guide

The [`guides/agentic-bus-migration.md`](../guides/agentic-bus-migration.md)
is for the *minority* of cases where moving an existing
dispatch + chain pipeline onto the bus is the right call. It
opens with a "when *not* to migrate" section because the bus is
additive and most pipelines don't need to move.

Four patterns covered with concrete before/after code:

1. **Fan-out dispatch** → topic + subscriptions.
2. **Tool-using agent loop** → conversation + tool envelopes.
3. **Streaming output** → stream envelopes + SSE consume.
4. **High-risk action** → tool-call with `require_approval: true`.

Each section names what you gain and what you pay so the
reader can make the trade-off without committing to the
refactor first.

## The multi-agent demo

`multi_agent_demo` is the runnable counterpart to the user
guide. Three agents (`planner-1`, `calendar`, `summarizer`)
share one private conversation. Over the course of the demo:

1. Planner posts `calendar.list_events` — calendar emits the
   matching `ToolResult` (Phase 6a).
2. Planner posts `text.summarize` — summarizer streams the
   answer as `StreamChunk`s and closes with
   `StreamEnd { complete }` (Phase 6b).
3. Planner posts a sensitive `billing.refund` — it parks under
   a `BusApproval`; an operator approves; the produced record
   carries an `acteon.approval.id` audit header; the resulting
   tool-result lands (Phase 6c).

It runs against the in-memory backend so no Kafka or HTTP
server is required:

```text
cargo run -p acteon-simulation --features bus --example multi_agent_demo
```

The same flow ports unchanged to the REST surface and the
polyglot SDKs (Rust, Python, Node, Go, Java).

## The benchmarks

The Criterion suite at
`crates/simulation/benches/bus_overhead.rs` measures what
Acteon's typed envelope layer costs *before* the message hits
the wire. Reference numbers (Apple M-series, release build,
in-memory backend):

| Bench | Median time | Throughput |
|---|---:|---:|
| `bus/raw_publish` | ~360 ns | ~2.78 M ops/s |
| `bus/post_tool_call` | ~1.68 µs | ~596 K ops/s |
| `bus/post_tool_result` | ~1.73 µs | ~577 K ops/s |
| `bus/post_stream_chunk` | ~1.28 µs | ~782 K ops/s |
| `bus/post_stream_end` | ~1.06 µs | ~945 K ops/s |
| `bus/validate/tool_call` | ~15 ns | — |

The full methodology + production-capacity implications live in
[`reference/bus-benchmarks.md`](../reference/bus-benchmarks.md).
The headline finding: the typed envelope layer adds ~700 ns to
~1.3 µs of in-process overhead per envelope vs a raw publish.
That's well below broker latency on any real Kafka deployment;
the bus's typed surface is not a bottleneck on realistic agent
workloads.

## Bus master plan: complete

With Phase 9 the master plan stops being a roadmap. Every
phase has shipped:

| Phase | Scope | Status |
|---|---|---|
| 1 | bus crate, topics, publish, subscribe SSE | shipped |
| 2 | subscriptions, ack, lag, DLQ | shipped |
| 3 | schemas, publish-edge validation | shipped |
| 4 | agents, heartbeat, shared inbox | shipped |
| 5 | conversations, threads, replay | shipped |
| 6a | tool-call envelopes | shipped |
| 6b | streaming chunks | shipped |
| 6c | HITL pre-publish approvals | shipped |
| 7 | admin UI | shipped |
| 8 | polyglot SDKs (Rust, Python, Node, Go, Java) | shipped |
| 9 | docs, migration guide, multi-agent demo, benchmarks | shipped |

What lives on past Phase 9 — operator-driven follow-ups, not
new master plan phases:

- **Kafka transactional producer + outbox** for atomic HITL
  parking. V1 documents the non-atomic trade-off; the
  follow-up closes the window.
- **Authenticated agent identity.** V1 takes `as_agent` as an
  operator-asserted query parameter under the tenant grant. A
  future iteration derives identity from the API-key grant.
- **`PendingBusApprovals` index population.** The state-store
  key kind is reserved; a background reaper would populate it
  to scale the approvals queue beyond a few hundred rows.
- **Consumer-side benchmark.** The Phase 9 bench measures the
  producer path; a `bus_kafka_e2e` bench against the
  `KafkaBackend` would round out the picture against a real
  broker.

These are tracked separately and won't be Phase 10.
