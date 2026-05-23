# Agentic Bus — Phase 4

> **Scope:** `Agent` identity + capabilities + heartbeat + shared inbox
> topic. Conversations and tool-call envelopes land in Phase 5/6. See
> the [master plan](../concepts/bus-master-plan.md).

Phase 1–3 gave us topics, durable subscriptions, and schemas. Phase 4
makes the bus *agent-aware*: the operator registers an agent once, gets
a stable identity with capabilities and heartbeat, and can address
messages to that agent by id without managing a topic per agent.

## What ships in Phase 4

| Surface | Shape |
|---|---|
| Core type | `acteon_core::Agent` — `{agent_id, namespace, tenant, display_name, capabilities, inbox_topic, heartbeat_ttl_ms, last_heartbeat_at, labels, created_at, updated_at}`; derived `AgentStatus` |
| State | `KeyKind::BusAgent`; key id = `agent_id` |
| Shared inbox | Default `{namespace}.{tenant}.agents-inbox`, auto-created on first agent registration. All agents in the same `(namespace, tenant)` share it; messages are keyed by `agent_id` so Kafka gives per-agent FIFO for free. |
| HTTP | `POST/GET /v1/bus/agents`, `GET|PUT|DELETE /v1/bus/agents/{ns}/{t}/{id}`, `POST /v1/bus/agents/{ns}/{t}/{id}/heartbeat`, `POST /v1/bus/agents/{ns}/{t}/{id}/send` |
| Rust client | `register_bus_agent`, `list_bus_agents`, `get_bus_agent`, `update_bus_agent`, `delete_bus_agent`, `heartbeat_bus_agent`, `send_to_bus_agent` |
| Tests | 11 core unit tests (status derivation, validation, serde roundtrip, inbox defaults + overrides) |
| Simulation | `bus_agent_simulation.rs` — register → discover by capability → status transitions → send → consume from shared inbox |

## Model

### Shared inbox, keyed delivery

Rather than giving each agent its own topic (which would explode Kafka
metadata and ACLs), all agents in a tenant share one inbox topic and
Acteon keys every message by `agent_id`. Kafka's default partitioner is
deterministic on key, so each agent's traffic lands on one partition —
per-agent FIFO ordering with zero subscription bookkeeping.

```
agents.demo.agents-inbox      (partitions = 3)
├── partition 0 ── agent_id=planner-1   msg msg msg
├── partition 1 ── agent_id=ocr-svc      msg msg
└── partition 2 ── agent_id=planner-2   msg msg msg
```

Subscribers filter locally by the `acteon.agent.id` header
(server-stamped) or the Kafka key. A fleet-wide consumer reads the
entire inbox and dispatches; a single-agent consumer reads from its
assigned partition.

Operators can override `inbox_topic` per agent if they want isolation
(for example, a sensitive-data agent that shouldn't share a topic with
the general pool). Cross-tenant inboxes are rejected at registration.

### Heartbeat-derived status

Agents POST to `/heartbeat` periodically. Status is computed on every
read:

| Age since last heartbeat | Status |
|---|---|
| no heartbeat yet | `Unknown` |
| within `heartbeat_ttl_ms` | `Online` |
| within `2 * heartbeat_ttl_ms` | `Idle` |
| older | `Dead` |

No background reaper, no TTL index, no timers. Dead records stay in
state until explicitly deleted.

## API shape

### Register

```http
POST /v1/bus/agents
{
  "agent_id": "planner-1",
  "namespace": "agents",
  "tenant": "demo",
  "display_name": "Planner One",
  "capabilities": ["planning", "reasoning"],
  "heartbeat_ttl_ms": 60000
}
→ 201 AgentResponse { inbox_topic: "agents.demo.agents-inbox", status: "unknown", ... }
```

First registration in `(namespace, tenant)` auto-creates the shared
inbox topic in both state and Kafka.

### Heartbeat

```http
POST /v1/bus/agents/agents/demo/planner-1/heartbeat
→ 200 { "agent_id": "planner-1", "last_heartbeat_at": "...", "status": "online" }
```

Cheap and idempotent. A common cadence is every `ttl / 3`.

### List + filter

```http
GET /v1/bus/agents?namespace=agents&tenant=demo&capability=ocr&status=online
→ 200 { "agents": [...], "count": 1 }
```

Capability matching is exact; a future phase can add semantic
matching. Status filter uses the derived status, so results reflect
the current heartbeat window.

### Send

```http
POST /v1/bus/agents/agents/demo/planner-1/send
{ "payload": { "task": "break down user request" } }
→ 200 { "inbox_topic": "agents.demo.agents-inbox", "agent_id": "planner-1", "partition": 0, "offset": 17, ... }
```

The server keys the Kafka record by `agent_id` and stamps
`acteon.agent.id` as a header. Callers can supply additional
non-reserved headers via `headers`.

## SDK example

```rust
use acteon_client::{ActeonClient, RegisterBusAgent, SendToBusAgent};

let client = ActeonClient::new("http://localhost:3000")?;

// Register.
let agent = client.register_bus_agent(&RegisterBusAgent {
    agent_id: "planner-1".into(),
    namespace: "agents".into(),
    tenant: "demo".into(),
    capabilities: vec!["planning".into()],
    ..Default::default()
}).await?;

// Heartbeat loop (run on a timer in your process).
client.heartbeat_bus_agent("agents", "demo", "planner-1").await?;

// Send.
client.send_to_bus_agent("agents", "demo", "planner-1", &SendToBusAgent {
    payload: serde_json::json!({"task": "summarize doc"}),
    ..Default::default()
}).await?;
```

## Authorization

All agent endpoints flow through `BusOp::ManageAgent` (requires the
`Dispatch` permission and a grant for verb `agent` on the target
`(tenant, namespace)`).

`send_to_agent` additionally checks `BusOp::Publish`, so operators
can hand out registry-read access without letting the same caller
blast messages into inboxes.

## Design decisions

- **Shared inbox, not per-agent topic.** Locked in the master plan —
  fewer topics to govern, partition utilization stays high, and
  Kafka's key-based partitioning gives us per-agent ordering without
  any subscription gymnastics.
- **Status on read, no reaper.** Simplicity wins in V1. If operators
  want proactive cleanup of dead records, a future phase can add a
  reaper; the current model keeps the core small.
- **Heartbeat endpoint, not "liveness inferred from send".** Sending
  a message doesn't mean the agent is reading its inbox. Keep the
  signals separate.
- **Inbox overrides stay tenant-scoped.** An agent can point at a
  dedicated topic, but only within its own `(namespace, tenant)`.

## How to try it

```bash
# Standalone — no Kafka required.
cargo run -p acteon-simulation --features bus --example bus_agent_simulation
```

For end-to-end HTTP, start the server with the bus feature and use the
SDK methods above.

## What comes next (Phase 5)

- `Conversation` type: per-conversation partitioning on a shared
  events topic, thread UI, conversation state machines.
- HITL gate for agent-initiated actions (pre-publish approval of
  messages the agent wants to send to other agents).
