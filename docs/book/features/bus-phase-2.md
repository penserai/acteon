# Agentic Bus — Phase 2

> **Scope:** durable subscriptions, manual offset commits, per-partition
> lag reporting, and dead-letter-queue routing. Schemas, agents,
> conversations, and tool-call envelopes come later. See the
> [master plan](../concepts/bus-master-plan.md).

Phase 1 gave us topics + publish + SSE subscribe. Phase 2 adds the
surfaces operators actually run the bus with: durable consumer groups
with committed offsets that survive reconnects, lag monitoring for
alerting, and DLQ routing so poison-pill messages don't wedge a
partition.

## What ships in Phase 2

| Surface | Shape |
|---|---|
| Core type | `acteon_core::Subscription` — `{id, topic, namespace, tenant, starting_offset, ack_mode, dead_letter_topic, ack_timeout_ms, labels}` |
| State | `KeyKind::BusSubscription` |
| Backend trait | `BusBackend::commit_offset` and `BusBackend::consumer_lag` added; both backends (Kafka + in-memory) implement them with consistent `committed = last consumed offset` semantics |
| HTTP | `POST/GET /v1/bus/subscriptions`, `DELETE /v1/bus/subscriptions/{namespace}/{tenant}/{id}`, `POST /v1/bus/subscriptions/{namespace}/{tenant}/{id}/ack`, `GET /v1/bus/subscriptions/{namespace}/{tenant}/{id}/lag`, `POST /v1/bus/subscriptions/{namespace}/{tenant}/{id}/deadletter` |
| Rust client | `create_bus_subscription`, `list_bus_subscriptions`, `delete_bus_subscription`, `ack_bus_subscription`, `get_bus_lag`, `deadletter_bus_message` |
| Tests | 2 new bus unit tests (commit + lag roundtrip, commit-to-missing-topic), 1 new Kafka integration test (commits survive reconnect) |
| Simulation | `bus_subscription_simulation.rs` — end-to-end: produce, consume + DLQ one record, reconnect, observe resumption and lag |

## API shape

### Create subscription

```http
POST /v1/bus/subscriptions
{
  "id": "order-processor",
  "topic": "agents.demo.orders",
  "namespace": "agents",
  "tenant": "demo",
  "starting_offset": "earliest",
  "ack_mode": "manual",
  "dead_letter_topic": "agents.demo.orders-dlq",
  "ack_timeout_ms": 30000
}
→ 201 SubscriptionResponse
```

### Ack

```http
POST /v1/bus/subscriptions/agents/demo/order-processor/ack
{
  "partition": 0,
  "offset": 42
}
→ 200 { "committed": true }
```

`offset` is the **last consumed** offset. The bus commits `offset + 1`
to Kafka so a fresh consumer in the same group starts at `offset + 2`.

> **Performance warning:** each `ack` call performs a full Kafka
> JoinGroup/SyncGroup round-trip (hundreds of milliseconds on a warm
> broker). It is **not** suitable for per-record acks in a
> high-throughput workload — batch or ack-at-end-of-batch only. A
> future phase will introduce a stateful subscription registry that
> keeps one long-lived consumer alive and routes commits through it.

### Lag

```http
GET /v1/bus/subscriptions/agents/demo/order-processor/lag
→ 200 {
  "subscription_id": "order-processor",
  "topic": "agents.demo.orders",
  "partitions": [
    { "partition": 0, "committed": 42, "high_water_mark": 50, "lag": 7 }
  ],
  "total_lag": 7
}
```

`committed: -1` indicates the consumer group has never committed on that
partition. `lag` is clamped at 0.

### Dead-letter

```http
POST /v1/bus/subscriptions/agents/demo/order-processor/deadletter
{
  "partition": 0,
  "offset": 42,
  "reason": "schema validation failed",
  "key": "order-42",
  "payload": { ... original payload ... },
  "headers": { "x-trace-id": "..." }
}
→ 200 { "dlq_topic": "...", "partition": 0, "offset": 0 }
```

The bus appends diagnostic headers (`acteon.dlq.origin_topic`,
`acteon.dlq.origin_partition`, `acteon.dlq.origin_offset`,
`acteon.dlq.subscription`, `acteon.dlq.reason`) before producing the
DLQ record. User-supplied `acteon.*` headers are filtered out at the
`BusMessage` layer.

## Tenant scoping in URLs

Subscription-scoped endpoints (`ack`, `lag`, `delete`, `deadletter`) all
carry `{namespace}/{tenant}/{id}` in the path. This shape has two
properties we rely on:

1. **O(1) state lookup.** The bus looks up subscriptions by exact
   `StateKey` (`/namespace/tenant/BusSubscription/id`) instead of
   scanning all subscriptions.
2. **Explicit tenant surface.** Callers can't accidentally address a
   subscription under the wrong tenant just because it has a matching
   `id`. Every subscription-scoped call authorizes against the
   `(namespace, tenant)` in the URL, matching the topic model.

At creation time, the server additionally validates that the
subscription's `topic` (and optional `dead_letter_topic`) belong to the
same `(namespace, tenant)` as the subscription itself, and that both
topics are governance-registered in the state store. Cross-tenant
subscriptions are rejected with `400 cross-tenant topic subscription
not allowed`.

## Known limitation — `commit_offset` semantics

Kafka only lets a consumer commit offsets for a group if that consumer
is *currently* a member of the group. The Phase 2 `commit_offset` API
spins up its own short-lived consumer, which means it can't join while
another consumer is still attached. Practical pattern:

1. Consume records through `BusBackend::subscribe`.
2. **Drop the subscribe stream** (so the consumer leaves the group).
3. Call `commit_offset` — Phase 2 will transparently spin up a new
   consumer, JoinGroup, commit, and leave.

This is fine for ack-at-end-of-batch workflows and for the
"drain-and-checkpoint" pattern. It's **not** suitable for
fine-grained per-record commits while the consumer is still attached
— a future phase introduces a stateful subscription registry that
holds one long-lived consumer and routes commits through it.

See `crates/simulation/examples/bus_subscription_simulation.rs` for
the canonical usage.

## Semantics: `committed` is "last consumed"

Both backends agree: `committed = N` means records 0..=N have been
processed; next to consume is `N + 1`. When the caller supplies
`offset = N` to `ack`, the bus commits `N + 1` to Kafka (Kafka's
convention is "next offset to read"). The `/lag` endpoint normalizes
back so callers see the same `committed` number they sent in.

## How to try it

```bash
docker compose --profile kafka up -d
ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
  cargo run -p acteon-simulation --features bus \
  --example bus_subscription_simulation
```

## What comes next (Phase 3)

- JSON Schema registry (`acteon_core::Schema`, CRUD endpoints).
- Publish-edge validation: bind a topic to a schema subject+version;
  reject payloads that don't match.
- Typed decoding helpers in the SDK.
