# Agentic Bus — Phase 6b

> **Scope**: typed `StreamChunk` / `StreamEnd` envelopes layered on
> conversation messages, with header-based stream identity and an SSE
> consume endpoint that closes on the terminal record. HITL pre-publish
> approvals land in 6c. See the [master plan](../concepts/bus-master-plan.md).

Phase 6a gave the bus a typed request/response protocol; Phase 6b adds
the natural complement — *progressive output*. LLM token streams,
partial tool results, progressive search hits, anything that follows
the "produce incremental, signal completion" pattern now has a typed
envelope and a header-filtered SSE consumer. Same architectural pattern
as 6a: tool-style envelopes ride the existing conversation events
topic, not a parallel pipeline.

## What ships in Phase 6b

| Surface | Shape |
|---|---|
| Core types | `acteon_core::StreamChunk`, `acteon_core::StreamEnd`, `acteon_core::StreamEndStatus ∈ {Complete, Aborted, Error}` |
| HTTP | `POST /v1/bus/conversations/{ns}/{t}/{id}/stream-chunks`, `POST .../stream-end`, `GET /v1/bus/streams/{ns}/{t}/{conv_id}/{stream_id}` (SSE) |
| Headers | Server-stamped: `acteon.envelope.kind ∈ {stream_chunk, stream_end}`, `acteon.stream.id`, `acteon.stream.seq`. Reuses Phase 5's reserved-prefix machinery. |
| Rust client | `post_bus_stream_chunk`, `post_bus_stream_end`, `bus_stream_consume_url` |
| Tests | 8 core unit tests covering envelope validation + serde roundtrips |
| Simulation | `bus_stream_simulation.rs` — producer streams 5 token chunks plus a terminal `complete`, consumer scans the events topic, header-filters by stream id, reassembles in chunk-seq order |

## Model

### Layered, not parallel

Stream envelopes are conventions on top of conversation messages —
identical to Phase 6a:

```
agents.demo.conversations-events
├── partition 1 ── conv=storytelling-thread
│       ├─ {kind=stream_chunk, stream.id=story-1, stream.seq=0}
│       ├─ {kind=stream_chunk, stream.id=story-1, stream.seq=1}
│       ├─ {kind=stream_chunk, stream.id=story-1, stream.seq=2}
│       └─ {kind=stream_end,   stream.id=story-1, status=complete}
```

Same partitioning rules (`key = conversation_id`), same audit, same
replay, same governance. Per-conversation Kafka ordering means a
single-conversation stream is naturally FIFO; `chunk_seq` is mostly
diagnostic in that case but becomes load-bearing when consumers
fan-out across partitions or stream from multiple conversations.

### `chunk_seq` and ordering

`chunk_seq` is operator-asserted, monotonic, non-negative. The bus
does not enforce monotonicity — a producer that emits out of order
will round-trip out-of-order chunks to consumers. Consumers that need
total order should sort by `chunk_seq` after collection (the
simulation example does this).

The terminal `StreamEnd` typically uses `chunk_seq = last + 1`, but
this is convention only. Consumers should detect end-of-stream via the
`acteon.envelope.kind == stream_end` header, not via a sequence-number
heuristic.

### `StreamEndStatus`

| Status | Meaning |
|---|---|
| `complete` | All chunks delivered. The happy path. |
| `aborted` | Producer canceled cleanly (operator stop, user canceled). Distinct from `error` so consumers can retry-or-surface differently. |
| `error` | Stream failed mid-flight. `error_message` carries detail. |

`error_message` is unconditionally capped at 4096 bytes — even on
`complete`. A producer that ships a megabyte of `error_message`
alongside `status: complete` would otherwise bypass the cap; same
trust-the-payload-not-the-status reasoning as Phase 6a's `ToolResult`.

### SSE consume

`GET /v1/bus/streams/{ns}/{t}/{conv_id}/{stream_id}` opens an SSE
connection. The handler:

1. Resolves the conversation's events topic (honors a custom
   `events_topic` set during `create_conversation`).
2. Reads the events topic via `BusBackend::scan_topic` (Kafka
   `assign()`, no consumer-group leak).
3. Header-filters on `acteon.envelope.kind ∈ {stream_chunk, stream_end}`
   AND `acteon.stream.id == stream_id`. Payload deserialization
   doesn't run on records that fail the filter.
4. Emits an SSE event per match: `event: bus.stream.chunk` for chunks,
   `event: bus.stream.end` for the terminal record.
5. Closes the connection cleanly when a `stream_end` for the target
   stream is observed.

A 15-second `keep-alive` pings the client between chunks so idle
streams don't trip intermediate proxies.

### Resume cursors

`POST /stream-chunks` and `POST /stream-end` both return a `cursor`
encoding the chunk's `partition:offset`. Pass the cursor returned
from chunk 0 to a consumer (via `?cursor=...` on the SSE URL) so it
scans from strictly after the chunk lands rather than from the topic
tail (the default — cheap on a busy cluster, but races with chunks
landing).

Same encoding as Phase 5 replay cursors and Phase 6a tool-result
lookup cursors: URL-safe base64 of a JSON `{partition: offset}` map,
capped at 8 KB.

## API shape

### Post a stream chunk

```http
POST /v1/bus/conversations/agents/demo/storytelling-thread/stream-chunks
{
  "stream_id": "story-1",
  "chunk_seq": 0,
  "body": {"token": "Once "},
  "sender": "storyteller-1"
}
→ 200 StreamEnvelopeReceipt {
  "events_topic": "agents.demo.conversations-events",
  "conversation_id": "storytelling-thread",
  "stream_id": "story-1",
  "chunk_seq": 0,
  "partition": 0,
  "offset": 17,
  "produced_at": "...",
  "cursor": "eyIwIjogMTd9"
}
```

### Post the terminator

```http
POST /v1/bus/conversations/agents/demo/storytelling-thread/stream-end
{
  "stream_id": "story-1",
  "chunk_seq": 5,
  "status": "complete",
  "sender": "storyteller-1"
}
→ 200 StreamEnvelopeReceipt
```

For a failure terminator:

```http
{
  "stream_id": "story-1",
  "chunk_seq": 3,
  "status": "error",
  "error_message": "upstream gave up",
  "sender": "storyteller-1"
}
```

### Consume

```http
GET /v1/bus/streams/agents/demo/storytelling-thread/story-1?cursor=eyIwIjogMTd9
Accept: text/event-stream
→ 200 OK

event: bus.stream.chunk
id: 17
data: {"stream_id":"story-1","chunk_seq":0,"body":{"token":"Once "},...}

event: bus.stream.chunk
id: 18
data: {"stream_id":"story-1","chunk_seq":1,"body":{"token":"upon "},...}

...

event: bus.stream.end
id: 22
data: {"stream_id":"story-1","chunk_seq":5,"status":"complete",...}

(connection closes)
```

For a private conversation (non-empty `participants`), pass
`?as_agent=<agent_id>` to identify which participant the consumer is
acting as. Same V1 read-isolation model as Phase 5 / 6a.

## SDK example

```rust
use acteon_client::{
    ActeonClient, BusStreamEndStatus, PostBusStreamChunk, PostBusStreamEnd,
};
use serde_json::json;

let client = ActeonClient::new("http://localhost:3000");

// Producer streams chunks.
for (seq, tok) in ["Once ", "upon ", "a ", "time."].iter().enumerate() {
    client.post_bus_stream_chunk("agents", "demo", "storytelling-thread",
        &PostBusStreamChunk {
            stream_id: "story-1".into(),
            chunk_seq: seq as i64,
            body: json!({"token": tok}),
            sender: Some("storyteller-1".into()),
            ..Default::default()
        }
    ).await?;
}

// Terminator.
client.post_bus_stream_end("agents", "demo", "storytelling-thread",
    &PostBusStreamEnd {
        stream_id: "story-1".into(),
        chunk_seq: 4,
        status: BusStreamEndStatus::Complete,
        error_message: None,
        sender: Some("storyteller-1".into()),
        metadata: Default::default(),
    }
).await?;

// SSE consume URL — wire it to your preferred client (browser
// EventSource, eventsource-stream, curl -N, etc.). The Rust client
// exposes the URL builder; SSE parsing is handled by the consuming
// runtime.
let url = client.bus_stream_consume_url(
    "agents", "demo", "storytelling-thread", "story-1",
);
```

## Authorization

All endpoints flow through `BusOp::ManageConversation`. The two
`POST` paths additionally require `BusOp::Publish`; the SSE consume
requires `BusOp::Subscribe`. Mirrors the Phase 5/6a split — operators
can lock down stream production independently from consumption.

Participant ACL applies to both write paths: when the conversation
has a non-empty `participants` list, the envelope's `sender` is
required and must be on the list. Read-side ACL on `consume_stream`
gates on `as_agent`, matching Phase 6a's lookup-result behavior.

## Design decisions

- **One pipeline, not two.** Stream envelopes ride the conversation
  events topic. No new Kafka topic, no new state-store rows for
  in-flight streams, one set of operational levers.
- **Server-stamped routing headers.** Subscribers route on a single
  header lookup; payload deserialization happens only on the chunks
  the caller actually wants. The same header-filter discipline that
  kept Phase 6a tool lookup cheap on a busy topic applies here.
- **SSE stops on terminator.** No client-side bookkeeping needed —
  observing `stream_end` for the target stream closes the connection.
  Producers that abandon streams without emitting `stream_end` will
  leave consumers hanging until the keep-alive eventually fails;
  document and lint accordingly.
- **`error_message` capped unconditionally.** Same lesson as Phase 6a:
  a per-status cap creates a bypass via `status: complete`. One cap
  for all statuses keeps the trust model simple.

## Trust model and limits

### `stream_id` is not a uniqueness gate

Two producers racing on the same `(conversation_id, stream_id)` will
interleave chunks on the topic. The bus does not enforce single-writer
semantics for a stream. In practice the participant ACL bounds the
threat — only conversation participants can produce to a private
thread — and operators control the participant list. If you need
mutually-exclusive producers, lease `stream_id` ownership at the
application layer.

### `chunk_seq` is not validated

The bus does not check monotonicity, contiguity, or that
`StreamEnd.chunk_seq == last_chunk + 1`. Consumers that need total
order should re-sort after collection. Consumers that need to detect
holes (a missing `chunk_seq`) should track the high-water mark and
raise a gap alarm at the application layer.

### Producers that abandon streams

A producer that emits chunks but never emits `StreamEnd` leaves
consumers with an open SSE connection until the underlying scan
errors out. This is a producer bug, not a bus bug — the bus has no
way to distinguish "stream still in flight" from "producer crashed."
For long-running streams, consider an upstream watchdog that emits
`StreamEnd { status: aborted }` on producer death.

## What comes next

- **Phase 6c** — HITL pre-publish approvals: park a tool-call (or
  stream chunk) in state, await operator approval, then commit to
  Kafka.
- **Phase 7** — UI: stream viewer in the conversation detail page,
  with live-tail and replay-from-cursor.
- **Phase 8** — 5-SDK parity for the bus surface (Python, Node, Go,
  Java).
