# Agentic Bus — Phase 1

> **Scope:** transport + topic CRUD + publish + subscribe. Consumer
> groups with persistent offsets, schema registry, agent/conversation
> primitives, typed tool-call envelopes, and HITL pre-publish gates
> are deferred to later phases. See the [master plan](../concepts/bus-master-plan.md).

## What ships in Phase 1

| Surface | Shape |
|---|---|
| New crate | `acteon-bus` (rdkafka-backed, with an in-memory backend for tests) |
| Core type | `acteon_core::Topic` — namespace + tenant + name + partitions/replication/retention |
| State | `KeyKind::BusTopic` — topic metadata persisted in the existing state store |
| Server config | `[bus]` section with `enabled` + `[bus.kafka]` |
| Server feature | `acteon-server/bus` Cargo feature (off by default) |
| HTTP | `POST /v1/bus/topics`, `GET /v1/bus/topics`, `DELETE /v1/bus/topics/{kafka_name}`, `POST /v1/bus/publish`, `GET /v1/bus/subscribe/{subscription_id}?topic=...&from=...` |
| Rust client | `create_bus_topic`, `list_bus_topics`, `delete_bus_topic`, `publish_message` |
| Docker | `kafka` profile in `docker-compose.yml` (KRaft single-broker on `localhost:9092`) |
| Tests | 7 bus unit tests (in-memory backend), 2 Kafka integration tests (gated on `ACTEON_KAFKA_BOOTSTRAP`) |
| Simulation | `crates/simulation/examples/bus_simulation.rs` — 2 competing agents + a tail consumer against a real broker |

## Kafka topic naming

`Topic::kafka_topic_name()` = `{namespace}.{tenant}.{name}`. Each
fragment is `[a-zA-Z0-9_-]{1..=80}`; dots are reserved as separators.
Tenant isolation is enforced at the transport layer — two tenants
asking for the same short name end up with different Kafka topics and
cannot read each other's messages.

## Dispatch edge (publish)

```text
POST /v1/bus/publish
{
  "topic": "agents.demo.inbox-xyz",      // or namespace/tenant/name triple
  "key": "partition-key",                // optional
  "payload": { "seq": 42 },
  "headers": { "x-trace-id": "..." }     // acteon.* prefix is rejected
}

→ 200 { "topic", "partition", "offset", "produced_at" }
```

Rules / quotas / audit still apply: Phase 1 publishes go through the
same rule-evaluation stage as actions (the handler accepts the
message, evaluates, then hands off to `BusBackend::produce`). The
per-publish receipt carries the broker-assigned partition and offset
so callers can correlate ack in their own audit logs.

## Consume edge (subscribe)

```text
GET /v1/bus/subscribe/{subscription_id}?topic=...&from=earliest|latest

→ text/event-stream
  event: bus.message
  id: <offset>
  data: { "topic", "key", "payload", "headers", "partition", "offset", "timestamp" }
```

Phase 1 subscriptions are **ephemeral** — the `subscription_id` is
used as a Kafka `group.id` for the duration of the connection, but no
offset is committed. Reconnects with `from=earliest` replay the retained
log; Phase 2 introduces durable `Subscription` objects with committed
offsets.

> **Important SSE semantics.** Because `subscription_id` maps directly
> to Kafka's `group.id`, two clients connecting to
> `/v1/bus/subscribe/{subscription_id}` with the **same ID** will have
> the topic's partitions **load-balanced** across them (Kafka's normal
> consumer-group rebalance). Each record goes to exactly one of them —
> this is *not* a broadcast fan-out. If you want every client to see
> every record, pick a distinct `subscription_id` per client. A
> broadcast-style subscription is a Phase 2 feature and will be
> surfaced as a dedicated flag on `Subscription`.

## Dual-write reconciliation

Topic CRUD is a dual-write against two systems (Acteon state store +
Kafka broker). Phase 1 keeps these sequential with loud error logging
on partial failures:

- **`POST /v1/bus/topics`** — state row first, then Kafka. If Kafka
  fails, Acteon deletes the state row (best-effort rollback). If the
  rollback also fails, we log at `error` with the full context so an
  operator can reconcile.
- **`DELETE /v1/bus/topics/{name}`** — state row first, then Kafka.
  Better to orphan in Kafka than in Acteon (operators can delete
  orphaned Kafka topics; dangling Acteon rows block re-creation). The
  Kafka failure is logged at `error` with the topic name.

Phase 2+ may introduce a background reconciler that scans the state
store and syncs with Kafka's admin API. Not needed for Phase 1.

## HITL interactions (Phase 1)

Pre-publish HITL (approval-park-then-produce) is **not in Phase 1** —
it's Phase 5, where we can design the Kafka-transaction + outbox
pattern properly. Until then the existing silencing/quota/rule
machinery still gates publish requests because every publish goes
through the same pipeline that `/v1/dispatch` uses.

## How to try it

```bash
# 1. Spin up Kafka.
docker compose --profile kafka up -d

# 2. Run the offline simulation (uses the real broker at localhost:9092).
ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
  cargo run -p acteon-simulation --features bus \
  --example bus_simulation

# 3. Or start the server with the feature and hit the HTTP API directly.
ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
  cargo run -p acteon-server --features bus -- \
  --config path/to/acteon.toml
```

Example `acteon.toml`:

```toml
[bus]
enabled = true

[bus.kafka]
bootstrap_servers = "localhost:9092"
client_id = "acteon-prod"
produce_timeout_ms = 5000
```

## What comes next (Phase 2)

- `Subscription` + `ConsumerGroup` as first-class types with durable
  offsets and DLQ routing.
- `/lag` endpoint for per-subscription replica monitoring.
- Explicit `ack`/`commit` endpoints so consumers can checkpoint.
