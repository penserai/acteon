# Agent2Agent (A2A) Protocol

Acteon implements the [A2A Protocol v1.0](https://a2aprotocol.org) — the
ratified standard for one agent to delegate work to another over a
durable Task abstraction. Bringing A2A into Acteon means agents you
own and agents your peers run can interoperate without per-counterparty
glue.

The whole protocol surface (JSON-RPC + REST + SSE) is scoped to one
namespace and one tenant via the URL prefix `/a2a/{namespace}/{tenant}`,
so the same multi-tenant model you use elsewhere in Acteon carries
through unchanged.

## When to reach for A2A

| Reach for A2A when | Reach for the native Acteon API when |
|---|---|
| You need to interop with non-Acteon agents | You only call Acteon-from-Acteon |
| The work is a long-running Task with intermediate state | The work is a single fire-and-forget dispatch |
| External clients should be able to subscribe to progress | Only internal subscribers consume the stream |
| You want to publish discovery metadata at `.well-known/agent.json` | Discovery is internal-only |

A2A and the rest of the Acteon API coexist on the same server. Both
share the same auth, the same audit trail, and the same gateway.

## URL surface

Every endpoint lives under `/a2a/{namespace}/{tenant}/…`. All endpoints
require the standard `Dispatch` permission grant on `(namespace,
tenant)` *except* the unauthenticated discovery endpoint.

### JSON-RPC 2.0 (the primary transport)

A single endpoint dispatches by method name in the envelope:

```text
POST /a2a/{namespace}/{tenant}
Content-Type: application/json

{ "jsonrpc": "2.0", "method": "tasks/get", "params": {…}, "id": 1 }
```

| Method | Purpose |
|---|---|
| `message/send` | Start a new Task or continue an existing one. |
| `tasks/get` | Read a Task by id. |
| `tasks/cancel` | Cancel a non-terminal Task. |
| `tasks/pushNotificationConfig/set` | Register a webhook for Task events. |
| `tasks/pushNotificationConfig/get` | Read one push config. |
| `tasks/pushNotificationConfig/list` | List push configs for a Task. |
| `tasks/pushNotificationConfig/delete` | Remove a push config. |
| `agent/getAuthenticatedExtendedCard` | Discovery card variant for authenticated callers. |

Batch arrays and notifications (requests with no `id`) follow the
JSON-RPC 2.0 spec exactly.

### REST binding (A2A spec §11)

Mirrors the JSON-RPC methods on resource-shaped paths:

```text
POST   /a2a/{ns}/{tenant}/v1/message:send
GET    /a2a/{ns}/{tenant}/v1/tasks/{id}
POST   /a2a/{ns}/{tenant}/v1/tasks/{id}:cancel
GET    /a2a/{ns}/{tenant}/v1/tasks/{id}/events            # SSE
POST   /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs
GET    /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs
GET    /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}
DELETE /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs/{cfgId}
```

### Discovery (unauthenticated)

```text
GET    /a2a/{ns}/{tenant}/.well-known/agent.json
```

Returns the tenant's `AgentCard` — verbatim if exactly one agent has
published, aggregated if several have. See [Discovery](#discovery)
below.

### Version negotiation

Every request and response carries an `A2A-Version` header. A request
that pins an unsupported version is rejected up front with JSON-RPC
error `-32000`. The current server speaks `1.0`.

## Tasks — the core abstraction

An A2A Task is a row in Acteon's state store with an explicit state
machine. The protocol defines eight states:

```text
              ┌─ Working ──┬─→ InputRequired ─┐
              │            ├─→ AuthRequired ──┤
   Submitted ─┘            │                  │
                           └──────┐           │
              ┌── Rejected ───────┤           │
              │                   ▼           ▼
              │             Completed | Canceled | Failed
```

| State | Meaning | Reachable from |
|---|---|---|
| `Submitted` | New Task; no work started yet. | (initial) |
| `Working` | Agent is actively processing. | `Submitted`, `InputRequired`, `AuthRequired` |
| `InputRequired` | Agent paused awaiting user input. | `Working` |
| `AuthRequired` | Agent paused awaiting user auth. | `Working` |
| `Completed` | Terminal, success. | `Working` |
| `Canceled` | Terminal, explicit cancel. | any non-terminal |
| `Failed` | Terminal, error or stale reap. | any non-terminal |
| `Rejected` | Terminal, never accepted (validation fail). | (initial; no row written) |

Acteon's Task Engine (`crates/gateway/src/task_engine.rs`) is the
single source of truth for these transitions. Illegal transitions
return a JSON-RPC `INVALID_PARAMS` error and leave the row unchanged.

### Send your first message

```bash
curl -X POST https://acteon.example.com/a2a/agents/demo \
  -H 'Authorization: Bearer YOUR_KEY' \
  -H 'Content-Type: application/json' \
  -H 'A2A-Version: 1.0' \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "message/send",
    "params": {
      "message": {
        "messageId": "01HPM-…",
        "role": "user",
        "parts": [{ "text": "Summarize the latest deploy log." }]
      }
    }
  }'
```

The response is the freshly-minted Task with `status.state = "submitted"`
and `history[0]` set to the inbound message.

### Continue an existing Task

Set `params.message.taskId` to thread the message into an existing
Task's history. The Task's state machine governs whether the
continuation is legal.

```json
{
  "messageId": "01HPM-…-b",
  "taskId": "task-alpha",
  "role": "user",
  "parts": [{ "text": "Yes, send it to ops@example.com." }]
}
```

### Cancel a Task

```bash
curl -X POST https://acteon.example.com/a2a/agents/demo/v1/tasks/task-alpha:cancel
```

The cancel verb is split off in-handler (axum routes whole segments).
Acteon also propagates the cancel to any linked Acteon Chain via the
Task ↔ Chain bridge.

## Pause for human (HITL)

Two of the eight Task states represent agent-initiated pauses:

- **`InputRequired`** — the agent needs the user to supply more
  information.
- **`AuthRequired`** — the agent needs the user to authenticate (e.g.
  re-authorize an OAuth grant).

A pause writes a `BusApproval` row alongside the Task transition in
one atomic step, so a pause never leaves an orphan approval and a
resolved approval is always paired with a Task move.

Resume by sending a normal `message/send` that drives the state
transition back to `Working`.

## Artifact streaming

Long-running Tasks emit results piecewise as `Artifact` rows. Each
artifact can be streamed across multiple `TaskArtifactUpdateEvent`s:

```text
chunk_index=0  last_chunk=false   "Hello, "
chunk_index=1  last_chunk=false   "world."
chunk_index=2  last_chunk=true    "!"
```

The artifact-stream gatekeeper enforces three cross-delivery invariants:

1. **No updates after `last_chunk=true`** — a "stream closed" attempt
   to append more is rejected.
2. **Strictly in-order `chunk_index`** — chunks must arrive 0, 1, 2, …
3. **Completeness on `total_chunks`** — when set, the stream cannot
   close with fewer chunks than promised.

The cap on Part content (text/raw/data) is **256 KiB**. Larger payloads
must use `Part::url` referencing an external store.

## SSE event subscription

Subscribe to per-Task lifecycle events:

```text
GET /a2a/{ns}/{tenant}/v1/tasks/{id}/events
Accept: text/event-stream
```

The stream emits three event types:

| Event | When |
|---|---|
| `task_transitioned` | After a successful `Submitted → Working`, `Working → Completed`, etc. |
| `task_history_appended` | After a `message/send` adds a history entry. |
| `task_artifact_updated` | After any `apply_artifact_update` commit. |

Each event carries `task_id`, plus event-specific fields (`from`/`to`
for transitions, `message_id` for history, `artifact_id`/`last_chunk`
for artifacts). The endpoint reuses the same `ConnectionRegistry` as
`/v1/stream` so per-tenant connection caps are unified.

**Live-only** — task events aren't persisted to audit, so
`Last-Event-ID` replay is intentionally not supported on this endpoint.

## Push notifications

For consumers that prefer push over pull, register a webhook with
`tasks/pushNotificationConfig/set`:

```bash
curl -X POST .../a2a/agents/demo \
  -d '{
    "jsonrpc": "2.0", "id": 1,
    "method": "tasks/pushNotificationConfig/set",
    "params": {
      "taskId": "task-alpha",
      "pushNotificationConfig": {
        "url": "https://my-service.example.com/a2a-hook",
        "token": "shared-secret"
      }
    }
  }'
```

The token (if set) is sent as `Authorization: Bearer <token>` on every
POST. The delivery worker:

- Subscribes to the same stream broadcast as the SSE endpoint.
- Looks up registered configs by task id (short-TTL cache folds
  bursts).
- POSTs the event envelope verbatim to every registered URL,
  concurrently across configs and across events.
- Retries with capped exponential backoff (1s, 2s, …, capped at 30s)
  for transient failures: HTTP 5xx, network/timeout errors, and **HTTP
  408 / 425 / 429** (the codes a well-behaved server uses to ask for
  backoff).
- Treats other HTTP 4xx as terminal — the URL is permanently rejecting
  the payload and the retry loop stops.
- Bounds total in-flight HTTP deliveries at 64, so a misbehaving
  destination cannot starve the rest of the fleet.

**Persistent DLQ.** Terminal failures (`4xx` other than `408/425/429`)
and exhausted-retry failures land in a per-tenant dead-letter queue
keyed by task. List, inspect, and purge entries via:

```text
GET    /v1/tasks/{task_id}/pushNotificationDLQ
GET    /v1/tasks/{task_id}/pushNotificationDLQ/{entry_id}
DELETE /v1/tasks/{task_id}/pushNotificationDLQ/{entry_id}
```

Counters for both delivery outcomes are also exported via
`PushDeliveryMetrics` and shown on the [Provider Health](provider-health.md)
dashboard.

### URL validation

Push URLs must be `http://` or `https://`. Other schemes (`file:`,
`javascript:`, etc.) are denied at registration time — the same
SSRF / information-disclosure surface that has bitten similar
webhook products. URLs are capped at 2 KiB.

## Discovery

Each tenant can publish an `AgentCard` (per-agent) that the
discovery endpoint exposes:

```text
GET /a2a/agents/demo/.well-known/agent.json
```

| Cards published | Card returned |
|---|---|
| 0 | 404 `not_found` |
| 1 | the single card, verbatim |
| ≥2 | a synthesized aggregate card (skills, interfaces, capabilities, schemes merged; colliding skill names suffixed `@agent_id`) |

The unauthenticated REST endpoint serves the discovery card. The
authenticated JSON-RPC method `agent/getAuthenticatedExtendedCard`
returns the same shape with `capabilities.extendedAgentCard = true`
set, so a client can confirm it is talking to the authenticated
extension. Future revisions may differentiate the two further.

### Security schemes on the card

When Acteon's auth is enabled, the discovery card is enriched with
the gateway's own intrinsic security schemes under reserved aliases:

| Alias | Scheme | Use |
|---|---|---|
| `acteon.bearer` | `HttpAuth { scheme = "bearer" }` | `Authorization: Bearer …` (JWT or API key) |
| `acteon.apiKey` | `ApiKey { name = "X-API-Key", in = "header" }` | Explicit API-key fallback header |

The enrichment is `entry().or_insert(…)` — if a tenant explicitly
publishes a scheme under either reserved alias, theirs is preserved
verbatim. `MutualTls` is not added intrinsically; a tenant using
mTLS publishes the scheme on its per-agent card.

## Limits and caps

| Limit | Value | Where |
|---|---|---|
| Request body | 2 MiB | `A2A_MAX_BODY_BYTES` |
| Part text / raw / data | 256 KiB | `MAX_PART_TEXT_BYTES`, etc. |
| Reference graph depth | 5 hops | `MAX_REFERENCE_DEPTH` |
| Reference graph width | 32 ids per message | `MAX_REFERENCE_TASK_IDS` |
| Working TTL (default) | 30 minutes | `DEFAULT_WORKING_TTL_MS` |
| Push URL | 2 KiB | `MAX_PUSH_URL_BYTES` |
| Push bearer token | 4 KiB | `MAX_PUSH_TOKEN_BYTES` |
| In-flight push deliveries | 64 concurrent | `MAX_INFLIGHT_DELIVERIES` |
| Push config cache TTL | 500 ms | `CONFIG_CACHE_TTL` |

## Try it locally

The simulation example exercises the entire surface in-process,
in ~200 ms:

```bash
cargo run -p acteon-simulation --example a2a_core_simulation
```

The output reads as a linear log with each scenario bracketed by a
banner. Engine actions and the `StreamEvent`s they emit line up by id.

## Errors

A2A error codes follow JSON-RPC 2.0 conventions plus the A2A-specific
codes from spec §8:

| Code | Name | When |
|---|---|---|
| `-32700` | `PARSE_ERROR` | The JSON-RPC body itself is malformed. |
| `-32600` | `INVALID_REQUEST` | The envelope is structurally wrong. |
| `-32601` | `METHOD_NOT_FOUND` | The method name is unknown. |
| `-32602` | `INVALID_PARAMS` | The params fail validation, *including* illegal Task transitions. |
| `-32603` | `INTERNAL_ERROR` | Server-internal failure (CAS exhaustion, state-store error). The message is intentionally opaque (CWE-209). |
| `-32001` | `TaskNotFound` | The named Task or sub-resource does not exist for this caller. |
| `-32002` | `TaskNotCancelable` | The Task is in a terminal state and cannot be canceled. |
| `-32000` | `VersionNotSupported` | The `A2A-Version` header names a version this server does not implement. |

REST responses translate these codes to HTTP statuses (`404`, `409`,
`400`, `500`, etc.) and put the message in the body.

## Architecture references

- Design doc: `docs/design/a2a-protocol.md`
- Task engine: `crates/gateway/src/task_engine.rs`
- Protocol codec (JSON-RPC + REST): `crates/server/src/api/a2a.rs`
- SSE consumer endpoint: `crates/server/src/api/a2a.rs::a2a_task_events`
- Discovery: `crates/server/src/api/a2a_discovery.rs`
- Push config CRUD: `crates/server/src/api/a2a_push.rs`
- Push delivery worker: `crates/server/src/api/a2a_push_worker.rs`
- Core types: `crates/core/src/bus_task.rs`, `bus_agent_card.rs`,
  `task_push.rs`
