# The Agentic Bus

> Operator-grade message bus for AI agents. Topics, agents,
> conversations, typed envelopes, streaming, HITL approvals — the
> primitives an agent fleet needs without rebuilding Kafka or
> writing protocol code from scratch.

This page is the user guide. It walks through the model end-to-end
so a new operator doesn't have to read all nine phase docs to
understand how the pieces fit. The phase-by-phase feature pages
remain the historical record for what each release shipped.

## Why a bus

Acteon already has `/v1/dispatch` for provider-executed actions
(send a Slack message, page the on-call, charge a card). That's
imperative: a caller picks an action, the gateway executes it,
done.

Agent fleets need a different model. Multiple long-running
processes need to:

- **Address each other** by stable identity (an agent's `agent_id`),
  not by a per-call URL.
- **Carry conversational context** across many tool-calls and
  responses on the same thread.
- **Stream partial output** — LLM tokens, progressive search hits.
- **Gate sensitive operations** behind a human decision before they
  execute.
- **Replay** to recover from a crash without losing in-flight
  state.

That's a message bus, not an RPC dispatcher. Acteon ships the
bus alongside dispatch. Both flow through rules, quotas, and
audit; only dispatch reaches the executor.

## The seven primitives

```
Topic ──┐
        │   ┌── Agent ── (heartbeat, capabilities, inbox topic)
        ├── Conversation ── (state machine, participant ACL, events_topic)
        │       ├── Plain message
        │       ├── ToolCall  ↔ ToolResult       (Phase 6a)
        │       ├── StreamChunk … StreamEnd       (Phase 6b)
        │       └── BusApproval (parked tool-call) (Phase 6c)
        │
        ├── Subscription ── (consumer group, ack-mode, lag, DLQ)
        └── Schema ── (publish-edge JSON Schema validation)
```

| Primitive | What it owns | When to reach for it |
|---|---|---|
| **Topic** | Kafka topic + Acteon metadata (name, partitions, retention, schema binding). | Long-lived event channels. Created once per workload. |
| **Schema** | A versioned JSON Schema bound to a topic. Validates every payload at publish time. | Hard contracts between producers and consumers. |
| **Subscription** | A consumer group with first-class identity (id, ack-mode, dead-letter-topic, lag reporting). | A consumer that needs operator-visible state. |
| **Agent** | Stable identity for a long-running process. Inbox topic + heartbeat + capabilities. | Anything that needs to be *addressed* on the bus. |
| **Conversation** | State-machine-bounded thread. Participant ACL, optional custom events topic. Per-conversation Kafka partitioning so ordering is FIFO within a thread. | Multi-turn sessions, planning loops, tool-using agents. |
| **Tool envelopes** | `ToolCall` / `ToolResult` typed records on the conversation events topic. Server stamps `acteon.envelope.kind` / `acteon.tool.call_id` / `acteon.correlation_id` headers. | Request/response between agents. |
| **Stream envelopes** | `StreamChunk` + `StreamEnd` per-chunk records with `acteon.stream.id` header. SSE consume endpoint that stops on the terminator. | LLM token streaming, progressive results. |
| **Approvals** | `BusApproval` row that parks a tool-call in Acteon state. Approve produces to Kafka; reject leaves no Kafka record. | Sensitive operations that need a human in the loop *before* execution. |

## Layered design

Tool envelopes, stream envelopes, and approvals all ride the
**same conversation events topic**:

```
agents.demo.conversations-events
├─ {kind=tool_call,    call_id=call-001,  sender=planner-1}
├─ {kind=tool_result,  call_id=call-001,  sender=calendar-svc}
├─ {kind=stream_chunk, stream.id=story-1, stream.seq=0}
├─ {kind=stream_chunk, stream.id=story-1, stream.seq=1}
├─ {kind=stream_end,   stream.id=story-1, status=complete}
└─ {kind=tool_call,    call_id=call-002,  approval.id=appr-1}
```

Same partitioning (`key = conversation_id`), same audit, same
replay. The typed envelopes are conventions on top of the wire
format, not a parallel pipeline.

Subscribers route on the `acteon.envelope.kind` header without
deserializing the payload. A consumer that only cares about
streaming chunks for one stream filters on
`(envelope.kind ∈ {stream_chunk, stream_end}, conversation.id, stream.id)`
and pays a hashmap-lookup cost for everything else.

## Endpoints at a glance

The full REST surface lives at `/v1/bus/*`. The five SDKs (Rust,
Python, Node, Go, Java) wrap these with identical method names.

```text
# Phase 1 — topics + publish
POST   /v1/bus/topics
GET    /v1/bus/topics
GET    /v1/bus/topics/{ns}/{tenant}/{name}
DELETE /v1/bus/topics/{ns}/{tenant}/{name}
POST   /v1/bus/publish

# Phase 2 — subscriptions + lag + DLQ
POST   /v1/bus/subscriptions
GET    /v1/bus/subscriptions
GET    /v1/bus/subscriptions/{id}
DELETE /v1/bus/subscriptions/{id}
GET    /v1/bus/subscriptions/{id}/lag
POST   /v1/bus/subscriptions/{id}/ack
POST   /v1/bus/subscriptions/{id}/dlq

# Phase 3 — schemas
POST   /v1/bus/schemas
GET    /v1/bus/schemas
GET    /v1/bus/schemas/{ns}/{tenant}/{subject}/{version}
DELETE /v1/bus/schemas/{ns}/{tenant}/{subject}/{version}

# Phase 4 — agents + heartbeat
POST   /v1/bus/agents
GET    /v1/bus/agents
GET    /v1/bus/agents/{ns}/{tenant}/{agent_id}
DELETE /v1/bus/agents/{ns}/{tenant}/{agent_id}
PATCH  /v1/bus/agents/{ns}/{tenant}/{agent_id}/heartbeat

# Phase 5 — conversations + replay + transition
POST   /v1/bus/conversations
GET    /v1/bus/conversations
GET    /v1/bus/conversations/{ns}/{tenant}/{id}
DELETE /v1/bus/conversations/{ns}/{tenant}/{id}
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/transition
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/messages   # append
GET    /v1/bus/conversations/{ns}/{tenant}/{id}/messages   # replay

# Phase 6a — tool-call envelopes
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/tool-calls
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/tool-results
GET    /v1/bus/tool-calls/{ns}/{tenant}/{call_id}/result   # blocking lookup

# Phase 6b — stream envelopes
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/stream-chunks
POST   /v1/bus/conversations/{ns}/{tenant}/{id}/stream-end
GET    /v1/bus/streams/{ns}/{tenant}/{conv_id}/{stream_id}  # SSE

# Phase 6c — HITL approvals
GET    /v1/bus/approvals/{ns}/{tenant}
GET    /v1/bus/approvals/{ns}/{tenant}/{approval_id}
POST   /v1/bus/approvals/{ns}/{tenant}/{approval_id}/approve
POST   /v1/bus/approvals/{ns}/{tenant}/{approval_id}/reject
```

## A complete tour

The `multi_agent_demo` simulation walks through every primitive
end-to-end with three agents (a planner, a calendar service, a
summarizer) sharing one conversation. It's runnable against the
in-memory backend so no Kafka is required:

```text
cargo run -p acteon-simulation --features bus --example multi_agent_demo
```

What it shows:

1. Agents register on the bus and join a private conversation
   (participant ACL enforced at envelope post).
2. Planner posts a tool-call; calendar emits the matching result.
3. Planner posts a summarize tool-call; summarizer streams the
   reply token-by-token plus a terminal `StreamEnd { complete }`.
4. Planner attempts a sensitive `billing.refund` call. It's
   parked under a `BusApproval`. Operator approves; the
   envelope produces with an `acteon.approval.id` audit header;
   the resulting tool-result lands.

The same flow ports unchanged to the REST surface — every step is
a single SDK call.

## What ships in each phase

If you need the deep dive on a specific area, jump to the phase
doc:

- **Phase 1** — [bus crate, topics, publish, subscribe SSE](../features/bus-phase-1.md)
- **Phase 2** — [subscriptions, ack, DLQ, lag](../features/bus-phase-2.md)
- **Phase 3** — [schemas, publish-edge validation](../features/bus-phase-3.md)
- **Phase 4** — [agents, heartbeat, shared inbox](../features/bus-phase-4.md)
- **Phase 5** — [conversations, threads, replay](../features/bus-phase-5.md)
- **Phase 6a** — [tool-call envelopes](../features/bus-phase-6a.md)
- **Phase 6b** — [streaming chunks](../features/bus-phase-6b.md)
- **Phase 6c** — [HITL pre-publish approvals](../features/bus-phase-6c.md)
- **Phase 7** — [admin UI](../features/bus-phase-7.md)
- **Phase 8** — [polyglot SDKs (Rust, Python, Node, Go, Java)](../features/bus-phase-8.md)

## Trust model summary

The full trust model lives across the phase docs (each phase
documents what the *new* surface does and doesn't enforce). The
recurring themes:

- **Single source of truth.** Wherever Acteon could maintain a
  parallel index that goes stale, it instead reads Kafka via
  `assign()` (no consumer-group leak) and header-filters. Replay
  paths and the typed-envelope lookup both work this way.
- **`call_id` / `correlation_id` / `chunk_seq` are operator-asserted.**
  The bus stamps what the producer claims as `acteon.*` headers
  but doesn't verify they're authentically linked. Sign payloads
  end-to-end if the participants don't trust each other.
- **Participant ACL gates write paths.** A conversation with a
  non-empty `participants` list rejects envelope posts whose
  `sender` isn't on the list. Read paths check the `as_agent`
  query parameter under the same rule.
- **HITL parking is not transactional (V1).** Approval row update +
  Kafka produce are two separate writes. Idempotent producer + the
  `acteon.approval.id` audit header keep the topic clean across
  retries; a Kafka transactional producer + outbox is the natural
  follow-up.

## Operational levers

| Need | Reach for | Surface |
|---|---|---|
| "Which consumers are falling behind?" | Lag dashboard | Phase 2 + Phase 7 UI |
| "Which agents are alive?" | Agent heartbeat | Phase 4 + Phase 7 UI |
| "What did this thread look like an hour ago?" | Conversation replay | Phase 5 |
| "Why was this tool-call's payload rejected?" | Schema binding + validation errors | Phase 3 |
| "Audit: who approved this refund?" | `BusApproval.decided_by` + `acteon.approval.id` Kafka header | Phase 6c |
| "Reconstruct the full LLM token stream" | Header-filter `(envelope.kind, stream.id)`, sort by `chunk_seq` | Phase 6b |

## When to use what

| Pattern | Use |
|---|---|
| Single-action provider call (send Slack, charge card) | `/v1/dispatch` (existing) |
| Multi-agent planning, tool use, conversation | Conversations + tool envelopes |
| LLM streaming (tokens, partial JSON) | Stream envelopes |
| Sensitive call needs human review | Tool envelope with `require_approval: true` |
| Long-lived event channel (logs, audit fan-out) | Topic + Subscription |
| Hard contract between producer/consumer | Topic + bound Schema |

The bus is **additive**. `/v1/dispatch` and chains stay; the
[migration guide](../guides/agentic-bus-migration.md) walks
through the cases where you might want to refactor a chain into a
bus flow.
