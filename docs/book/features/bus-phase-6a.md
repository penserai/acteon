# Agentic Bus — Phase 6a

> **Scope**: typed `ToolCall` / `ToolResult` envelopes layered on
> conversation messages, with `correlation_id` and `reply_to`
> semantics. Streaming chunks land in 6b; HITL pre-publish approvals
> in 6c. See the [master plan](../concepts/bus-master-plan.md).

Phase 5 made the bus thread-aware. Phase 6a makes it *protocol-aware*:
the dominant agent-to-agent pattern (call a tool, get a result) now
has typed envelopes, server-stamped routing headers, and a
"wait-for-result-by-id" lookup endpoint. No parallel Kafka pipeline —
tool envelopes ride on the existing conversation events topic.

## What ships in Phase 6a

| Surface | Shape |
|---|---|
| Core types | `acteon_core::ToolCall`, `acteon_core::ToolResult`, `acteon_core::ToolResultStatus ∈ {Ok, Error, Canceled}` |
| HTTP | `POST /v1/bus/conversations/{ns}/{t}/{id}/tool-calls`, `POST .../tool-results`, `GET /v1/bus/tool-calls/{ns}/{t}/{call_id}/result?conversation_id=&timeout_ms=` |
| Headers | Server-stamped: `acteon.envelope.kind ∈ {tool_call, tool_result}`, `acteon.tool.call_id`, `acteon.correlation_id`, `acteon.reply_to`. Reuses Phase 5's reserved-prefix machinery. |
| Rust client | `post_bus_tool_call`, `post_bus_tool_result`, `lookup_bus_tool_result` |
| Tests | 9 core unit tests covering envelope validation + serde roundtrips |
| Simulation | `bus_tool_call_simulation.rs` — caller produces a tool-call, responder produces a result, caller recovers it via the same header-filter primitive the lookup endpoint uses |

## Model

### Layered, not parallel

Tool envelopes are conventions on top of conversation messages:

```
agents.demo.conversations-events
├── partition 1 ── conv=planning-thread
│       ├─ {kind=tool_call,    call_id=call-001, sender=planner-1}
│       ├─ {kind=tool_result,  call_id=call-001, sender=calendar-svc}
│       ├─ {kind=tool_call,    call_id=call-002, sender=planner-1}
│       └─ {kind=tool_result,  call_id=call-002, sender=ocr-svc}
```

Same partitioning rules (`key = conversation_id`), same audit, same
replay, same governance — operators don't see a new pipeline. The
typed envelopes are an addition to the wire format, not a replacement.

### `correlation_id` and `reply_to`

Both are first-class envelope fields. The bus stamps them as headers
so subscribers can route locally without parsing JSON:

- **`correlation_id`** — opaque token a caller chooses to thread
  request and response together across hops. The matching
  `ToolResult` mirrors it. Subscribers filter on
  `acteon.correlation_id`.
- **`reply_to`** — when a caller wants the result to land in a
  *different* conversation than the call (a fan-out dispatcher
  collecting results in a dedicated thread, for example), they set
  this on the `ToolCall`. The bus stamps `acteon.reply_to`; the
  responder honors it by posting their `tool-result` to that thread.
  Empty / unset = "same conversation".

### Wait-for-result lookup

`GET /v1/bus/tool-calls/{ns}/{t}/{call_id}/result` blocks for up to
`timeout_ms` waiting for a matching `ToolResult` to land on the
events topic. Internally:

1. Reads the relevant events topic via `BusBackend::scan_topic`
   (Kafka `assign()`, no consumer-group leak).
2. Header-filters on `acteon.envelope.kind == tool_result` AND
   `acteon.tool.call_id == call_id`. Only payloads that pass the
   header filter get deserialized.
3. Returns the typed `ToolResult` plus bus coordinates
   (`partition`, `offset`, `produced_at`, `events_topic`,
   `conversation_id`).
4. On timeout, returns `408 Request Timeout` so retrying clients
   know they didn't miss the result — they just need to wait longer
   or check again.

`?conversation_id=` overrides the default-tenant events topic when
the caller routed the result to a different thread via `reply_to`.

## API shape

### Post a tool-call

```http
POST /v1/bus/conversations/agents/demo/planning-thread/tool-calls
{
  "call_id": "call-001",
  "tool": "calendar.list_events",
  "arguments": {"day": "2026-04-28", "limit": 5},
  "sender": "planner-1",
  "correlation_id": "trace-42"
}
→ 200 ToolEnvelopeReceipt {
  "events_topic": "agents.demo.conversations-events",
  "conversation_id": "planning-thread",
  "call_id": "call-001",
  "partition": 0,
  "offset": 17,
  "produced_at": "..."
}
```

### Post a tool-result

```http
POST /v1/bus/conversations/agents/demo/planning-thread/tool-results
{
  "call_id": "call-001",
  "status": "ok",
  "output": {"events": [...]},
  "sender": "calendar-svc",
  "correlation_id": "trace-42"
}
→ 200 ToolEnvelopeReceipt
```

### Wait for a result

```http
GET /v1/bus/tool-calls/agents/demo/call-001/result?timeout_ms=5000
→ 200 ToolResultLookupResponse {
  "call_id": "call-001",
  "result": {"status": "ok", "output": {...}, ...},
  "events_topic": "...",
  "partition": 0,
  "offset": 18,
  ...
}
```

`408 Request Timeout` if no result lands in `timeout_ms`. Default 5s,
max 30s.

## SDK example

```rust
use acteon_client::{
    ActeonClient, BusToolResultLookupParams, BusToolResultStatus,
    PostBusToolCall, PostBusToolResult,
};

let client = ActeonClient::new("http://localhost:3000")?;

// Caller emits a tool call.
client.post_bus_tool_call("agents", "demo", "planning-thread",
    &PostBusToolCall {
        call_id: "call-001".into(),
        tool: "calendar.list_events".into(),
        arguments: serde_json::json!({"day": "2026-04-28"}),
        sender: Some("planner-1".into()),
        correlation_id: Some("trace-42".into()),
        ..Default::default()
    }
).await?;

// Responder emits the result.
client.post_bus_tool_result("agents", "demo", "planning-thread",
    &PostBusToolResult {
        call_id: "call-001".into(),
        status: BusToolResultStatus::Ok,
        output: serde_json::json!({"events": [...]}),
        sender: Some("calendar-svc".into()),
        correlation_id: Some("trace-42".into()),
        ..Default::default()
    }
).await?;

// Caller waits for the result.
let lookup = client.lookup_bus_tool_result("agents", "demo", "call-001",
    &BusToolResultLookupParams {
        timeout_ms: Some(5_000),
        ..Default::default()
    }).await?;
assert_eq!(lookup.result.call_id, "call-001");
```

## Authorization

All endpoints flow through `BusOp::ManageConversation` (Dispatch
permission, verb `conversation`). The two `POST` paths additionally
require `BusOp::Publish`, mirroring `append_conversation_message` —
operators can split read/write ACLs.

Participant ACL applies to `tool-calls` and `tool-results`: when the
conversation has a non-empty `participants` list, the envelope's
`sender` is required and must be on the list. Same gate as
`append_conversation_message`.

## Design decisions

- **Layered, not parallel.** Tool envelopes are typed conventions
  over conversation messages — one pipeline, one source of truth,
  one set of operational levers.
- **Server-stamped routing headers.** Subscribers route on a single
  header lookup; payload deserialization happens only on the
  envelopes the caller actually wants.
- **Lookup via topic scan, not a side index.** Same trade-off as
  conversation replay (Phase 5): one source of truth, no consistency
  window. A future secondary index makes sense once tool-call
  volume is high enough that scan latency dominates.
- **Result fetch returns 408 on timeout, not 404.** The result may
  arrive any moment; 408 is the honest signal "not yet, retry."

## Trust model and limits

The bus does not maintain a state-store record of in-flight tool
calls (the layered design keeps the call/result envelopes on Kafka,
not in Acteon state). That has two consequences operators should
plan around:

### `call_id` is not a uniqueness gate

`lookup_tool_result` returns the **first** record on the topic that
matches `(envelope.kind=tool_result, tool.call_id=<id>)`. Acteon
does not verify that only one responder produced a result for that
call. If two participants race to post results for the same
`call_id`, the lookup returns whichever the broker stored first.

In practice the participant ACL bounds the threat: only conversation
participants can post tool results, and operators control who's on
the list. A mutually-untrusting set of participants in the same
thread is unusual; if you have one, sign your tool-result payloads
end-to-end and verify on the consumer side.

### `correlation_id` is operator-asserted

The bus does not verify that a `ToolResult.correlation_id` matches
the originating `ToolCall.correlation_id`. The server stamps
whatever the responder claims as `acteon.correlation_id`. This is
fine for tracing — the field is informational and audited — but
treat it as the responder's claim, not as an authenticated link.

If you need an authenticated request/response link, sign payloads
or use a mutually-trusted ID generator and verify on the consumer.

### Read-side participant ACL

The write-side handlers gate on `sender ∈ participants`. The
read-side (`lookup_tool_result`, `replay_conversation_messages`)
gates on `as_agent ∈ participants` when the conversation has a
non-empty list. Pass `as_agent` to identify which participant
you're acting as. Without `as_agent` on a private thread, the
server returns 403.

`as_agent` is operator-asserted under the tenant grant — anyone with
the tenant grant can claim any agent identity in the tenant. A
future iteration will derive the agent identity from the API-key
grant directly so this can't be spoofed.

## What comes next

- **Phase 6b** — Streaming chunks (per-chunk Kafka records with a
  terminal marker) for LLM token streams and partial tool results.
- **Phase 6c** — HITL pre-publish approvals: park a tool-call in
  state, await operator approval, then commit to Kafka.
- **Future** — derive agent identity from API-key grant (replaces
  the `as_agent` self-assertion); state-store-tracked `call_id`
  uniqueness for environments where participant ACL isn't enough.
