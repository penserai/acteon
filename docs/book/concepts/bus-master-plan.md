# Agentic Message Bus — Master Plan

This doc is the north star for converting Acteon into a message bus specialized for
agentic orchestration. It locks architectural decisions, lays out all 9 phases, and
documents the model. Phase-level progress links live at the bottom.

> **Feature gate:** the bus feature is compiled behind `acteon-server/bus` Cargo
> feature (opt-in, same as `swarm`). Builds without the feature compile unchanged
> and respond to `/v1/topics` / `/v1/publish` / `/v1/subscribe/*` with `503`.

## Problem framing

Acteon already has ~60–70 % of a bus latent in its machinery: an action dispatch
pipeline with rules, quotas, audit, retries, DLQ, circuit breakers, signing, SSE
streaming, and 5 polyglot SDKs. What it lacks for agentic workloads is durable
topic-based subscriptions, typed envelopes, agent-as-actor primitives,
conversation threading, and streaming replies.

Rather than reinvent Kafka, we **use Kafka as the transport/canonical log** and
build Acteon-native primitives on top for the agentic semantics Kafka doesn't
care about.

## Locked architectural decisions

1. **Kafka is canonical.** Messages live on Kafka topics. Acteon's audit store is
   a searchable projection, not a parallel source of truth.
2. **Schema registry is Acteon-native.** JSON Schema in V1; Avro later. No
   dependency on Confluent Schema Registry.
3. **Agent inbox model.** Agents share one inbox topic keyed by `agent_id` rather
   than owning a topic each. Better partition utilization; simpler ACLs.
4. **Conversations** are a state object. Messages for a conversation land on a
   shared `conversations.events` topic partitioned by `conversation_id` so Kafka
   gives us per-conversation ordering for free.
5. **HITL gates are pre-publish.** Approvals park the message in Acteon state
   (not Kafka) until approved, then commit via a Kafka transaction. This is the
   hardest corner; see "Exactly-once edge" below.
6. **Streaming = per-chunk Kafka records.** `stream_id` + `chunk_seq` + terminal
   marker. One primitive for LLM tokens, tool-result streams, partial updates.
7. **Actions and Chains stay.** Boundary:
   - `POST /v1/dispatch` = provider-executed action (unchanged).
   - `POST /v1/publish` = bus-only event (no provider). Both flow through rules
     and quotas; only dispatch reaches the executor.
8. **Feature-gated.** `bus` Cargo feature; off by default.

## Kafka integration model

| Concept            | Kafka owns                   | Acteon owns                                              |
|--------------------|------------------------------|----------------------------------------------------------|
| Transport          | Producer / broker / consumer | Validation, rules, quotas, HITL gate at publish edge     |
| Partition assignment | Consumer group rebalancing | Subscription **identity** (name, ACL, schema binding)    |
| Offsets            | `__consumer_offsets`         | Lag reporting, replay UX                                 |
| Retention          | `retention.ms` / `retention.bytes` | Policy metadata on `Topic` objects                 |
| Schema             | —                            | `Schema` objects + validation at publish edge            |
| Identity / ACL     | (optionally via SASL)        | API-key auth, tenant scoping, per-topic ACL             |

Rule of thumb: **if Kafka has a built-in primitive, we use it**; if Kafka is
agnostic (agent identity, schema semantics, conversation grouping), Acteon owns
it.

## Phased roadmap

| Phase | Weeks | Scope                                                                                          |
|-------|-------|------------------------------------------------------------------------------------------------|
| **1** | 1–3   | `acteon-bus` crate (rdkafka). `Topic` type + CRUD. `POST /v1/publish`. `GET /v1/subscribe/{id}` SSE bridge. `bus` Cargo feature. Docker Kafka profile. |
| 2     | 4–5   | `Subscription` + `ConsumerGroup` first-class types. Ack endpoint. DLQ routing. `/lag` endpoint. |
| 3     | 6–7   | JSON Schema registry. Publish-edge validation. SDK codecs.                                     |
| 4     | 8–9   | `Agent` type — identity, capabilities, heartbeat, inbox = shared topic keyed by `agent_id`.    |
| 5     | 10–11 | `Conversation` type. Per-conversation partitioning on shared events topic. Thread UI.          |
| 6     | 12–13 | `ToolCall` / `ToolResult` envelopes. `correlation_id` / `reply_to`. Streaming chunks. HITL tool-call approvals. |
| 7     | 14–15 | UI: Topics, Subscriptions, Agents, Conversations, Lag dashboards. Metrics.                     |
| 8     | 16–17 | 5-SDK parity for bus surface (Rust, Python, Node, Go, Java).                                    |
| 9     | 18    | Docs, migration guide, example multi-agent app, benchmarks vs raw Kafka.                       |

## Exactly-once edge

The pre-publish HITL gate introduces a "park then produce" window where intent
and publication must commit atomically. Design for Phase 5:

1. On publish with `require_approval`: write an `unpublished_message` row keyed
   by `approval_id` under an existing transaction with the approval row.
2. Approval handler, on approve, produces via a Kafka transaction that includes
   deleting the `unpublished_message` row (via transactional outbox pattern —
   we emit a companion "outbox-committed" record and have a cleanup worker
   reconcile).
3. Idempotent producer (`enable.idempotence=true`) + per-`(tenant,message_id)`
   dedup on Kafka side via a hash-based approach.

This section is deliberately deferred to Phase 5 design; Phase 1 does not need
transactional producers.

## Migration story

Phase 1 does not migrate anything. Existing users keep using `/v1/dispatch` with
no behavior change. The bus is additive.

Long-term, once all phases ship, a migration story emerges:
- **Fan-out dispatches** that multiple consumers need → rewrite as `Topic` +
  subscribers.
- **Chains** that are really event-driven pipelines → rewrite as topic-to-topic
  flows with agents subscribing.
- **Tool-calling apps** that use raw dispatch → rewrite as `ToolCall` envelopes
  with typed responses.

Migration will be opt-in, per-feature, never forced.

## Phase status

- **Phase 1** — in progress (PR #1)
- Phase 2–9 — not started

See [phase-1 feature doc](../features/bus-phase-1.md) for current-state details.
