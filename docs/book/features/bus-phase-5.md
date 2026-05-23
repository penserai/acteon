# Agentic Bus — Phase 5

> **Scope**: `Conversation` type, multi-agent threads on a shared
> events topic with Kafka-keyed FIFO ordering, lifecycle state machine,
> participant ACL, message append + replay. Tool-call envelopes and
> streaming chunks land in Phase 6. See the
> [master plan](../concepts/bus-master-plan.md).

Phases 1–4 gave us topics, durable subscriptions, schemas, and agents.
Phase 5 makes the bus *thread-aware*: operators register a
conversation once, agents post messages to it, and any reader can
replay the full history filtered to one thread — without provisioning
a Kafka topic per conversation.

## What ships in Phase 5

| Surface | Shape |
|---|---|
| Core type | `acteon_core::Conversation` — `{conversation_id, namespace, tenant, title, state, participants, events_topic, labels, created_at, updated_at}` |
| State | `KeyKind::BusConversation`; key id = `conversation_id` |
| Shared events topic | Default `{namespace}.{tenant}.conversations-events`, auto-created on first conversation registration. All conversations in the tenant share it; messages keyed by `conversation_id` so Kafka's partitioner gives per-thread FIFO ordering for free. |
| HTTP | `POST/GET /v1/bus/conversations`, `GET\|PUT\|DELETE /v1/bus/conversations/{ns}/{t}/{id}`, `POST /v1/bus/conversations/{ns}/{t}/{id}/transition`, `POST\|GET /v1/bus/conversations/{ns}/{t}/{id}/messages` |
| Rust client | `register_bus_conversation`, `list_bus_conversations`, `get_bus_conversation`, `update_bus_conversation`, `delete_bus_conversation`, `transition_bus_conversation`, `append_bus_conversation_message`, `replay_bus_conversation_messages` |
| Tests | 10 core unit tests covering the state machine, validation, inbox defaults + overrides, and serde roundtrip |
| Simulation | `bus_conversation_simulation.rs` — two conversations on one shared topic, replay with header filter, full state-machine walkthrough, participant ACL |

## Model

### Shared events topic, keyed messages

Same trick as the agent inbox: every conversation in a tenant shares
one Kafka topic. Acteon keys each message by `conversation_id`, so
Kafka's default partitioner deterministically routes one thread's
traffic to one partition — per-conversation FIFO ordering with no
subscription bookkeeping.

```
agents.demo.conversations-events       (partitions = 3)
├── partition 0 ── conversation_id=plan-001  msg msg msg
├── partition 1 ── conversation_id=rev-002    msg msg
└── partition 2 ── conversation_id=plan-007  msg msg msg
```

Replay reads the topic from the requested offset, filters on the
server-stamped `acteon.conversation.id` header, and bounds latency
with a small budget (default 1500ms / 200 messages, both tunable).

Operators can override `events_topic` per conversation if they want
isolation — for example, a high-volume thread that should not share a
topic with the general pool. Cross-tenant overrides are rejected at
registration.

### Lifecycle state machine

```text
   ┌─────────┐  resolve   ┌──────────┐  archive   ┌──────────┐
   │ Active  │───────────▶│ Resolved │───────────▶│ Archived │
   └─────────┘            └──────────┘            └──────────┘
         ▲                      │
         └─────── reopen ───────┘
```

- `Active` and `Resolved` accept message posts. A follow-up after
  resolution is a common pattern; the bus doesn't punish it.
- `Archived` is read-only by design. Reopen is rejected; the only
  way back is to register a new conversation.
- `Active → Archived` requires going through `Resolved` first. No
  shortcut.

### Participant ACL

Each conversation carries a `participants: Vec<String>` of agent IDs
who are allowed to post. Empty list means "open thread, any sender
accepted." When the list is non-empty, append-message rejects any
sender outside it with `400 Bad Request`. This is checked
**after** the standard publish authorization, so participant ACL is
strictly a *narrower* gate than the tenant's bus-write grant.

## API shape

### Register

```http
POST /v1/bus/conversations
{
  "conversation_id": "plan-001",
  "namespace": "agents",
  "tenant": "demo",
  "title": "Planning Q3",
  "participants": ["planner-1", "ocr-svc"]
}
→ 201 ConversationResponse {
  "events_topic": "agents.demo.conversations-events",
  "state": "active", ...
}
```

First registration in `(namespace, tenant)` auto-provisions the shared
events topic in both state and Kafka.

### Append message

```http
POST /v1/bus/conversations/agents/demo/plan-001/messages
{
  "payload": { "kind": "draft", "body": "Step 1..." },
  "sender": "planner-1"
}
→ 200 { "events_topic": "...", "conversation_id": "plan-001",
         "partition": 0, "offset": 17, "produced_at": "..." }
```

The server keys the Kafka record by `conversation_id` and stamps two
reserved headers: `acteon.conversation.id` and (if `sender` is set)
`acteon.conversation.sender`.

### Replay thread

```http
GET /v1/bus/conversations/agents/demo/plan-001/messages?from=earliest&limit=200&timeout_ms=1500
→ 200 {
  "conversation_id": "plan-001",
  "events_topic": "agents.demo.conversations-events",
  "messages": [
    { "partition": 0, "offset": 12, "key": "plan-001",
      "payload": {...}, "headers": {...}, "timestamp": "..." },
    ...
  ],
  "limit_reached": false
}
```

Each replay opens a one-shot consumer group (UUID-suffixed) so it
doesn't interfere with any durable subscription state. `limit_reached
= true` means more messages may exist past the returned tail; clients
paginate by re-issuing with a higher offset (or just request more
messages with a larger `limit`).

### State transitions

```http
POST /v1/bus/conversations/agents/demo/plan-001/transition
{ "transition": "resolve" }
→ 200 ConversationResponse { "state": "resolved", ... }
```

Illegal transitions return `409 Conflict` with the enum-described
disallowed move (e.g. `archive` from `active`).

## SDK example

```rust
use acteon_client::{
    ActeonClient, AppendBusConversationMessage, BusConversationTransition,
    RegisterBusConversation, ReplayBusConversationParams,
};

let client = ActeonClient::new("http://localhost:3000")?;

// Register.
let conv = client.register_bus_conversation(&RegisterBusConversation {
    conversation_id: "plan-001".into(),
    namespace: "agents".into(),
    tenant: "demo".into(),
    title: Some("Planning Q3".into()),
    participants: vec!["planner-1".into(), "ocr-svc".into()],
    ..Default::default()
}).await?;

// Append.
client.append_bus_conversation_message("agents", "demo", "plan-001",
    &AppendBusConversationMessage {
        payload: serde_json::json!({"kind": "draft", "step": 1}),
        sender: Some("planner-1".into()),
        ..Default::default()
    }
).await?;

// Replay.
let history = client.replay_bus_conversation_messages("agents", "demo", "plan-001",
    &ReplayBusConversationParams { ..Default::default() }).await?;
println!("thread has {} messages", history.messages.len());

// Resolve.
client.transition_bus_conversation(
    "agents", "demo", "plan-001",
    BusConversationTransition::Resolve,
).await?;
```

## Authorization

All conversation endpoints flow through `BusOp::ManageConversation`
(requires `Dispatch` permission and a grant for verb `conversation`
on the target `(tenant, namespace)`).

`POST /messages` additionally requires `BusOp::Publish`, so operators
can split read-only viewer roles from posting agents.

Participant ACL is enforced **after** the publish gate: even with
publish rights, a sender outside the participant list is rejected
with `400`.

## Atomic create

`register_conversation` uses `StateStore::check_and_set` for the
conversation row, mirroring the post-#132 pattern. Two concurrent
posts for the same `conversation_id` cleanly produce one `201` and
one `409` — no silent overwrite.

## Design decisions

- **Shared events topic, not per-conversation topic.** Locked by the
  master plan. Fewer topics, simpler ACLs, Kafka's key partitioner
  gives per-thread ordering. Override per-conversation if you need
  isolation.
- **Replay via topic scan + header filter.** One source of truth
  (Kafka), no parallel store, no consistency window. The latency
  budget bounds the cost.
- **Linear state machine with explicit `Reopen`.** No `Active →
  Archived` shortcut; mistakes happen, so `Resolved → Active` is
  allowed.
- **Participants on the conversation, not derived from message
  senders.** Lets operators ACL the thread before any agent posts.

## How to try it

```bash
# Standalone — no Kafka required.
cargo run -p acteon-simulation --features bus --example bus_conversation_simulation
```

For end-to-end HTTP, start the server with the bus feature and use the
SDK methods above.

## What comes next (Phase 6)

- `ToolCall` / `ToolResult` envelope types riding on top of
  `Conversation`.
- `correlation_id` / `reply_to` semantics so a tool-call request and
  its response thread together cleanly.
- Streaming chunks (per-chunk Kafka records with a terminal marker)
  for LLM token streams and partial tool results.
- HITL pre-publish approvals for sensitive tool-call envelopes.
