# Agent2Agent (A2A) Protocol

Acteon exposes the [A2A Protocol v1.0](https://a2a-protocol.org) JSON-RPC
binding over its durable Task abstraction. The public card and protocol objects
use the official ProtoJSON shapes, while Acteon keeps ownership, recovery and
execution metadata inside the platform. Existing pre-1.0 method names and the
older stored-card view remain available during migration.

The protocol has two deliberate addressing levels. The tenant endpoint at
`/a2a` is a durable interoperable Task service. The exact
agent endpoint at `/a2a/agents/{agent}` invokes one
operator-qualified agent runtime under Acteon's execution authority. An
aggregate tenant card is useful for discovery, but it is never used to choose a
runtime implicitly.

## When to reach for A2A

| Reach for A2A when | Reach for the native Acteon API when |
|---|---|
| You need to interop with non-Acteon agents | You only call Acteon-from-Acteon |
| The work is a long-running Task with intermediate state | The work is a single fire-and-forget dispatch |
| External clients should be able to subscribe to progress | Only internal subscribers consume the stream |
| You want to publish discovery metadata at `.well-known/agent-card.json` | Discovery is internal-only |

A2A and the rest of the Acteon API coexist on the same server. Both
share the same auth, the same audit trail, and the same gateway.

## URL surface

Every endpoint lives under `/a2a/…`. All endpoints
require the standard `Dispatch` permission grant on `(namespace,
tenant)` *except* the unauthenticated discovery endpoint.

### JSON-RPC 2.0 (the primary transport)

A single endpoint dispatches by method name in the envelope:

```text
POST /a2a
Content-Type: application/json

{ "jsonrpc": "2.0", "method": "GetTask", "params": {…}, "id": 1 }
```

| Method | Purpose |
|---|---|
| `SendMessage` | Start a new Task or continue an existing one. |
| `GetTask` | Read a Task by id. |
| `ListTasks` | List visible Tasks with filters and cursor pagination. |
| `CancelTask` | Cancel a non-terminal Task. |
| `GetExtendedAgentCard` | Discovery card variant for authenticated callers. |
| `SendStreamingMessage` | Start or continue a Task and receive official `StreamResponse` frames over SSE. |
| `SubscribeToTask` | Receive an initial durable Task snapshot followed by live status and artifact updates. |
| `CreateTaskPushNotificationConfig` | Register a governed Task webhook. |
| `GetTaskPushNotificationConfig` | Read one Task webhook configuration. |
| `ListTaskPushNotificationConfigs` | Page through a Task's webhook configurations. |
| `DeleteTaskPushNotificationConfig` | Remove a Task webhook configuration. |

Push configuration operations require an operator or administrator because
their responses can contain webhook tokens or authentication credentials. The
same control applies when `SendMessage` or `SendStreamingMessage` embeds a
push configuration.

The lowercase slash-separated methods remain compatibility aliases for existing
Acteon clients. New integrations should use the PascalCase v1 methods. A v1
`SendMessage` result contains exactly one of `task` or `message`; roles and task
states use ProtoJSON names such as `ROLE_USER` and `TASK_STATE_SUBMITTED`.
By default `SendMessage` waits until the Task is terminal or pauses for input or
authorization. Set `configuration.returnImmediately` to `true` when the caller
wants durable asynchronous acceptance and will poll with `GetTask`.
When `configuration.taskPushNotificationConfig` is present, Acteon validates it
before binding it to the Task ID created or recovered by `SendMessage`; callers
cannot bind that inline configuration to a different Task.
`ListTasks` supports `contextId`, ProtoJSON `status`, `statusTimestampAfter`,
`historyLength`, `includeArtifacts`, and page sizes from 1 through 100. Its
opaque page token is bound to those filters, and every page reports the
required `pageSize`, `totalSize`, and `nextPageToken` fields.

Batch arrays and notifications (requests with no `id`) follow the
JSON-RPC 2.0 spec exactly.

### Invoke a governed agent through official A2A

Publish an individual agent card whose JSON-RPC interface URL names the exact
agent endpoint:

```text
POST /a2a/agents/{agent}
```

`SendMessage` at this address does more than create a Task row. Acteon verifies
the current private caller credential, the source-to-agent grant, permits,
closures, registry qualification, destination credential, shared execution
budget, and the installed provider binding. It then persists the Task and both
execution contexts before the background driver may start the provider effect.
The agent ID comes only from the route and cannot be supplied or changed by a
model-generated message. An Acteon host can attach its privately held
`X-Acteon-Execution-Context` and `X-Acteon-Execution-Permits` headers to make
the call a bounded descendant of existing work; both headers are required as a
pair and message content cannot manufacture either one.

The governed endpoint implements `SendMessage`, `GetTask`, `ListTasks`, `CancelTask`,
the four Task push configuration methods, `GetExtendedAgentCard`,
`SendStreamingMessage`, and `SubscribeToTask`. A new message may set
`configuration.returnImmediately=true` to receive durable acceptance. A
continuation must target the same Task and active input challenge. `CancelTask`
first commits the same durable execution-subtree fence as the native `stop`
operation. If no provider attempt started, or a qualified reconciliation proof
establishes no effect, the returned Task is `TASK_STATE_CANCELED` and
`metadata.acteonCancellation.state` is `confirmed`. If an effect is already in
flight, unsupported by the provider, or uncertain, the method returns the actual
nonterminal Task with cancellation state `requested` and
`futureStartsBlocked=true`; it does not invent provider finality. A later known
provider result remains authoritative. Repeating cancellation of a canceled Task
is idempotent; other terminal Tasks return `TaskNotCancelable`.

`ListTasks` enumerates only durable acceptances for the selected agent and the
current authenticated source. It implements every A2A 1.0 filter, exact
`totalSize`, last-update-descending cursor pagination, history trimming, and
artifact selection. Page tokens are bound to the caller, optional source
receipt, and complete filter set. The runtime reads acceptances through bounded
pages on the configured state backend and rechecks authentication after the
scan. Agent principals must send one exact job receipt, so their result contains
at most that job; this prevents a shared agent credential from enumerating other
jobs. Human, service, and system principals can list all of their own Tasks for
the selected agent. Push configuration uses the same original-task observation
check and additionally requires operations-management permission. This applies
to create, secret-bearing get/list, delete, and inline registration. Agent
principals must present the exact source receipt; a human or service manager may
use another credential for the same original principal and credential authority.
The selected card must advertise `pushNotifications`.
This exact-agent endpoint accepts one
JSON-RPC request or notification per HTTP call; it does not accept batch arrays.

Admission returns `X-Acteon-Agent-Source-Context`, an opaque signed receipt.
Keep it outside prompts and model output. Human principals can later prove
ownership from their current credential alone. An agent principal must also
send this header on `GetTask` and continuation requests, preventing two jobs
that share one service identity from observing or joining each other. Native
Acteon SDK agent-service receipts retain this value automatically.

### Official A2A streaming

When a published card enables `capabilities.streaming`, the JSON-RPC endpoint
supports the v1 `SendStreamingMessage` and `SubscribeToTask` methods. The HTTP
response is `text/event-stream`; every SSE `data` record is a complete JSON-RPC
response carrying the original request id and one official `StreamResponse`
payload:

```text
data: {"jsonrpc":"2.0","id":41,"result":{"task":{…}}}
data: {"jsonrpc":"2.0","id":41,"result":{"statusUpdate":{…}}}
data: {"jsonrpc":"2.0","id":41,"result":{"artifactUpdate":{…}}}
```

The initial Task snapshot closes the subscribe-before-read race. Ordered
transition events then use ProtoJSON states. Both operations close at a terminal
state. `SendStreamingMessage` also closes at an input or authorization pause so
the caller can satisfy the interrupt; `SubscribeToTask` remains open through an
interrupt and closes when the Task becomes terminal. Artifact frames carry the
durable resolved artifact as a replacement (`append=false`), which lets a
client recover safely even if it fell behind the broadcast buffer. A lagged
consumer receives a fresh Task snapshot rather than a nonstandard warning
frame. The stream shares Acteon's tenant connection quota and releases its slot
on disconnect. On the exact-agent endpoint, every relevant event and lag
refresh revalidates the original authenticated source through the governed
runtime before emitting a frame. `SendStreamingMessage` returns the opaque
`X-Acteon-Agent-Source-Context` receipt as an HTTP response header, and all
Task snapshots omit Acteon's private execution binding.

`SubscribeToTask` rejects terminal Tasks with `UnsupportedOperation`; use
`GetTask` for their final snapshot. A card that does not advertise streaming
also returns `UnsupportedOperation`. Streaming calls must be single JSON-RPC
requests with an `id`, because one SSE response cannot multiplex a JSON-RPC
batch or notification.

Every native SDK invokes these methods directly. For example, TypeScript keeps
the source receipt beside each validated result instead of inserting it into
the Task or message:

```typescript
for await (const frame of client.agentServiceSendStreamingMessage(
  "prod", "acme", "notifier", message,
)) {
  saveHostProvenance(frame.sourceContext);
  render(frame.result);
}

for await (const frame of client.agentServiceSubscribeTask(receipt)) {
  render(frame.result);
}
```

Rust, Python, Go, and Java expose the corresponding
`agent_service_send_streaming_message`, `AgentServiceSendStreamingMessage`, or
`agentServiceSendStreamingMessage` methods and retained-Task subscription
methods. Each implementation validates the response media type, A2A version,
request ID, and a 2 MiB frame bound before exposing a result. Default stream
transports do not impose the finite-call timeout on a healthy long-lived
subscription.

### Acteon task and governed-peer REST routes

These resource-shaped routes support Acteon SDKs and governed peer operations.
They are not advertised as an A2A 1.0 `HTTP+JSON` interface in the public card:

```text
POST   /a2a/v1/message:send
GET    /a2a/agents/{agent}/v1/tasks/{id}/peers?skill={exact-skill}
POST   /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/message:send
POST   /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:refresh
POST   /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:cancel
POST   /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/message:send
POST   /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/authorization:resolve
POST   /a2a/agents/{agent}/v1/tasks/{id}/message:send
GET    /a2a/v1/tasks/{id}
POST   /a2a/v1/tasks/{id}:cancel
GET    /a2a/v1/tasks/{id}/events            # SSE
POST   /a2a/v1/tasks/{id}/pushNotificationConfigs
GET    /a2a/v1/tasks/{id}/pushNotificationConfigs
GET    /a2a/v1/tasks/{id}/pushNotificationConfigs/{cfgId}
DELETE /a2a/v1/tasks/{id}/pushNotificationConfigs/{cfgId}
```

The agent-service routes are Acteon's governed peer tools. Discovery returns
only source-approved, currently online registry entries whose installed binding
still matches the current card. It exposes a bounded agent ID, exact skill,
card version, binding digest, observation time, and optional untrusted
description. It never returns an endpoint, credential, permit, recipient
principal, or authority handle. The result is advisory; every send repeats all
current authority and binding checks.

A configured agent calls from one accepted source task and supplies only a message. The server
recovers the source context and permits, resolves the reviewed target binding and
credential, and journals the send before network delivery. Its response is
accepted, rejected, or uncertain; uncertainty is never an instruction to retry.
After acceptance, the refresh route rechecks the current source credential,
permit, grant, registry card and installed binding before reading the exact
remote task. It rejects identity changes and lifecycle regressions and retains
the last valid snapshot when the remote read is unavailable. Individual-agent
acceptance and task reads return a strong `ETag` derived from that task row's
version in the configured state backend. The source journals this opaque
progress cursor with the accepted remote snapshot and sends it on the next read
as `If-None-Match`. An unchanged target returns `304 Not Modified`; a changed
target returns the complete task and a replacement cursor. The cursor survives
source restart with the rest of the peer journal, and the source never accepts
the same cursor with different task content or a lifecycle regression.

The cancel route uses the same recovered authority and exact accepted send
record. Acteon journals cancellation intent and a delivery claim before making
one remote call to the target service's native stop route. `unsupported` and
`rejected` are definitive; `uncertain` means delivery may have occurred and is
never retried automatically. `restricted` proves the target durably fenced
future provider starts and carries its exact current nonterminal task. It does
not claim that an already-started provider effect stopped. A later explicit
cancel call may use safe task observation to return `reconciled` with the exact
terminal task. That terminal task can be completed, failed, canceled, or
rejected: reconciliation proves finality, while only the task state says whether
the cancellation took effect. If the target committed its restriction but that
response was lost, a restart preserves `uncertain`; an explicit repeat performs
one read and sends no second stop. A still-nonterminal task remains `uncertain`
because observation alone cannot reconstruct the lost restriction
acknowledgment. The complete two-server send, discovery, native stop, source
restart, and exact receipt-recovery contract is qualified against both Redis and
PostgreSQL through the same `StateStore` boundary.

When an accepted remote task pauses in `InputRequired`, the peer continuation
route accepts only an unbound user message. Acteon recovers the exact remote
task, context, challenge, previous progress cursor, credential and permits from
its durable journal, then binds them at the trusted HTTP boundary. The target
authenticates the original private source and resolves that exact challenge.
The receipt is `accepted`, `rejected`, or `uncertain`; accepted receipts include
the same task and a new strong progress cursor. Replaying the same response
after a source restart returns the durable receipt without appending the answer
twice. Changed content conflicts, and ambiguous delivery is never retried
automatically. This two-server contract runs against both Redis and PostgreSQL.

### Governed cross-domain federation

Acteon's federation boundary adds two protected routes for independently
administered Acteon domains:

```text
POST /a2a/federation/envelopes:import
POST /a2a/federation/agents/{agent}/v1/message:send
POST /a2a/agents/{source}/v1/tasks/{task}/federation/exports/{profile}/message:send
POST /a2a/agents/{source}/v1/tasks/{task}/federation/exports/{profile}/message:observe
POST /a2a/agents/{source}/v1/tasks/{task}/federation/exports/{profile}/message:replay
```

The first route authenticates a configured foreign peer, verifies its
audience-bound signed envelope against a current local trust revision, and
creates a bounded local authority root without starting a task or effect. The
second accepts an exact signed invocation, requires its embedded message to
match the request body, and invokes only the qualified local agent binding under
the imported ceiling. Concurrent exact replay converges on the retained task;
changed signed input conflicts.

The matching outbound transport journals the exact
signed envelope and invocation in the configured state backend before calling
either route, checkpoints import separately from invocation, and persists only
validated receipts and tasks. A missing or malformed response is `uncertain`
and a normal repeat does not resend it. Explicit recovery is limited to a
transport binding qualified for exact idempotent replay. Credentials and
network policy stay in the guarded host adapter and never enter durable delivery
state.

Source authority constructs those signatures from an
operator-qualified export profile and a matching host key. Before delivery it
charges the complete delegated remote unit ceiling to a current local
`federation.export` permit. Revocation before an unused network stage records a
local rejection without contacting the peer; ambiguous replay also requires
current source authority. The profile, endpoint, key, credential, permits and
effect ceiling are host inputs and cannot come from an agent message.

The three source routes recover the accepted local task and match it to current
private authentication. The caller supplies only the installed profile name and
message. Acteon allocates stable IDs, timestamps, expiry and an attenuated
remote budget in the configured state backend, so a retry or restart rebuilds
the same signatures. `message:observe` never calls the peer and remains useful
for accounting after revocation. `message:replay` is explicit, rechecks current
authority, and works only with the Acteon HTTP binding qualified for exact
idempotent replay.

Managers with independent `can_reconcile = true` authority can list a bounded
page of outbound delivery evidence:

```text
GET /v1/governance/federation/deliveries&limit=50
```

The cursor is the last returned delivery UUID. Each item identifies the source,
parent execution, message ID, remote agent, trust/key epoch, timestamps, expiry,
replay qualification, negotiated capability-card digest/key/epoch/protocol,
authentication scheme, qualified credential-provider revision, and exact
delivery stage. The view excludes the message
body, signed payloads, endpoint, transport credential and signing material. The
server filters results to the manager's declared source principals and
revalidates management authority after reading the state backend.
An active read-only card fetch appears as `capability_negotiating` with its
claim deadline; no card body or signature is exposed.

An authorized manager can terminally stop local recovery of an ambiguous or
expired claimed stage:

```text
POST /v1/governance/federation/deliveries/{delivery_id}:abandon
{
  "namespace": "prod",
  "tenant": "acme",
  "disposition_id": "77777777-7777-4777-8777-777777777777",
  "reason": "peer status cannot be proven; stop local recovery"
}
```

The disposition is idempotent and records the authenticated operator, current
authority generation, reason and time. It releases the local export reservation
after the abandonment is durable. The resulting status remains `abandoned`, not
`rejected`: Acteon is recording a local recovery decision and does not claim to
know whether the peer performed the effect. This management operation contains
no endpoint, credential or replay path. Replay still requires the original
source authority and a transport qualified for exact idempotency.

Before import, the guarded adapter fetches the protected peer capability route,
verifies the audience-bound Ed25519 card against the profile epoch, and requires
the exact agent binding, skill, protocol, authentication scheme, replay guarantee
and optional peer-finality guarantee. The journal commits sanitized negotiation
evidence before the import call. A card outage is safely retryable because no
effect endpoint was called; a signed downgrade or mismatch is a terminal local
import-stage refusal.

The production qualification contract runs two real Acteon processes, loses an
accepted invocation response, restarts both domains, and recovers the exact
remote task without repeating the provider effect. It runs once with isolated
Redis domains and again with a Redis source plus PostgreSQL destination. The
mixed-backend run also revokes destination trust and proves a later export is
rejected without another provider call. See the
[federated agent mesh guide](../guides/federated-agent-mesh.md).

### Discovery (unauthenticated)

```text
GET    /a2a/.well-known/agent.json
GET    /a2a/.well-known/agent-card.json
```

The `agent-card.json` route returns the schema-closed A2A 1.0 projection used by
official clients. It requires the published card to name a JSON-RPC interface
qualified for v1. The older `agent.json` route returns Acteon's stored-card view
for compatibility. Both aggregate several published cards at tenant scope.
The v1 card advertises push notifications when the published durable card
enables them. Streaming remains false until its v1 adapters pass the external
conformance gate; Acteon's existing SSE route remains available through the
native API in the meantime.

### Version negotiation

Every request and response carries an `A2A-Version` header. A request
that pins an unsupported version is rejected up front with JSON-RPC
error `-32009`. The current server speaks `1.0`.

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

The REST `message:send` and `tasks/{id}` responses include a strong `ETag`
for the exact task representation. Send that value as `If-None-Match` on a
later task read to receive `304 Not Modified` when the authoritative state row
has not changed. Treat the value as opaque. A `historyLength` projection has
its own representation-specific ETag, so a cursor from a full task read cannot
incorrectly validate a trimmed response.

### Send your first message

```bash
curl -X POST https://acteon.example.com/a2a \
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
curl -X POST https://acteon.example.com/a2a/v1/tasks/task-alpha:cancel
```

The cancel verb is split off in-handler (axum routes whole segments).
Acteon also propagates the cancel to any linked Acteon Chain via the
Task ↔ Chain bridge.

## Chain-backed Tasks

A Task can be **backed by an Acteon Chain**: the chain's progress
projects onto the Task's state (chain running → `Working`, terminal
chain status → the matching terminal Task state).

When the chain settles, its **step results fold into the Task** so a
federated caller gets the output through the standard protocol rather
than an opaque `Completed`:

- one **`Artifact`** per chain step — `artifactId` `step-{index}`,
  `name` the step name, with the step's response body as a data part
  (parallel sub-steps project as `substep-{name}`);
- a summary **`history`** message naming the chain and how many step
  results were recorded.

A step body larger than the 256&nbsp;KB part cap is replaced by a small
`{"truncated": true, …}` marker — the full body stays queryable through
the chain API. Projection is best-effort: an enrichment failure is
logged and never blocks the Task from settling.

## Pause for human (HITL)

Two of the eight Task states represent agent-initiated pauses:

- **`InputRequired`** — the agent needs the user to supply more
  information.
- **`AuthRequired`** — the agent needs the user to authenticate (e.g.
  re-authorize an OAuth grant).

A pause writes a `BusApproval` row and then compare-and-swap transitions the
Task. If the CAS acknowledgement is lost, Acteon reads the Task back and
accepts only the exact matching pause; a proven unused approval is removed
best-effort. A non-empty pause reason is also returned as the agent-authored
`status.message`, while `pendingApprovalId` names the exact challenge.

To answer `InputRequired`, send `message/send` with the same `taskId` and
`contextId`, role `user`, and echo `pendingApprovalId` as
`metadata["acteon.challengeId"]`:

```json
{
  "messageId": "response-01",
  "taskId": "task-alpha",
  "contextId": "incident-42",
  "role": "user",
  "parts": [{ "data": { "region": "us-west-2" } }],
  "metadata": { "acteon.challengeId": "019a…" }
}
```

The Task Engine binds the response to that exact pending approval, stores a
canonical content digest in the approval, appends the full response once to Task
history, and moves `InputRequired → Working`. The approval moves
`Pending → Approving → Approved`. An identical retry repairs an interruption
between the Task and approval writes without duplicating history. A stale
challenge ID, changed body under the same message ID, mismatched context, agent
role, or terminal Task is rejected without resuming work. Expiration closes the
approval and moves the still-paused Task to `Failed`; cancellation during the
two-row handoff closes the claimed approval instead of leaving it in
`Approving`.

Hosted governed agents continue through a separate provider execution for each
accepted challenge. Acteon records that intent before leaving the pause, derives
a deterministic child execution under the recipient's existing permit ceiling,
and sends the provider both the exact response and bounded conversation history.
The background driver recovers a registered continuation even if a crash left
the Task visibly paused. Identical concurrent calls converge on one provider
attempt; changed content or a different authenticated requester is refused.
Cross-server peer delivery uses the authenticated HTTP adapter and the same
durable source and target journals. The source sends only the exact normalized
response; host state supplies the task, context, challenge and conditional
cursor. Redis and PostgreSQL contracts cover delivery, source restart, receipt
recovery, and one history append.

`AuthRequired` has a separate trust boundary. Message content cannot grant
authority and is never treated as a credential. A configured hosted agent opens
the challenge with an opaque verifier request handle; Acteon supplies the
binding-qualified verifier, revision, recipient, credential authority, audience,
and scopes. The original requester resolves the returned challenge through the
host-installed verifier. The verifier credential never enters the task or
either public request body. An unbound legacy auth pause cannot be resumed.

```text
POST .../tasks/{id}/authorization:request   # authenticated recipient agent
POST .../tasks/{id}/authorization:resolve   # authenticated original requester
```

For a remote peer, the source calls the submission-scoped
`authorization:resolve` route with the exact challenge ID returned by peer
refresh. Acteon journals the handoff in the configured state backend before
contacting the target. The source never receives or forwards credentials,
verifier secrets, evidence, scopes, or an authorization request handle. A
denial is durable. If the target commits but its response is lost, a retry after
source restart observes the exact remote task and settles the journal without
calling the verifier again. Current credentials, permits, closures, service
binding, peer grant, and registry state are rechecked before every handoff or
reconciliation. A terminal failure or cancellation does not prove that the
authorization succeeded, so ambiguous handoffs remain `uncertain` in those
states.

```text
POST .../peers/{target}/{skill}/submissions/{submission}/authorization:resolve
body: { "challengeId": "<exact pending challenge>" }
```

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

Acteon's native event feed remains available for clients that want internal
event envelopes instead of official A2A `StreamResponse` frames:

```text
GET /a2a/v1/tasks/{id}/events
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
`Last-Event-ID` replay is intentionally not supported on this endpoint. After
an SSE disconnect, use the task `ETag`/`If-None-Match` contract to recover the
complete durable snapshot. That cursor validates task state; it does not claim
to replay every transient SSE envelope.

Official `SubscribeToTask` uses the same live broadcast but always begins with
the current durable Task snapshot, so reconnecting clients recover state before
receiving new updates.

## Push notifications

For consumers that prefer push over pull, register a webhook with the official
method on either the tenant Task service or an exact governed agent:

```bash
curl -X POST .../a2a/agents/notifier \
  -H 'A2A-Version: 1.0' \
  -d '{
    "jsonrpc": "2.0", "id": 1,
    "method": "CreateTaskPushNotificationConfig",
    "params": {
      "taskId": "task-alpha",
      "id": "incident-hook",
      "url": "https://my-service.example.com/a2a-hook",
      "tokenRef": {"provider": "environment", "key": "callbacks/notification-token", "version": "2026-10"},
      "authentication": {"scheme": "Bearer", "credentialsRef": {"provider": "environment", "key": "callbacks/incident-router", "version": "2026-10"}}
    }
  }'
```

The callback receives `Content-Type: application/a2a+json` and
`A2A-Version: 1.0`. Its body is an official status or artifact
`StreamResponse`. Acteon sends `token` in `X-A2A-Notification-Token`; it sends
the separately configured authentication scheme and credentials in
`Authorization`. A production callback should use host-owned `tokenRef` and
`credentialsRef` values, which are resolved only at delivery and remain safe in
state and management projections. See [secret custody](secret-custody.md).
Inline values remain available for protocol compatibility; secret-bearing
create, get, and list results require operations-management authority.

The delivery worker:

- Commits the official A2A v1 callback body in the same state-backend CAS as
  the Task mutation.
- Uses the live stream broadcast only as a low-latency wake-up. Bounded Task
  pages recover missed wake-ups after lag or restart. A state-backend ready-work
  index normally directs polling to Tasks whose next callback is due; workers
  rebuild missing or stale hints from the authoritative Task envelope.
- Claims each callback's next sequence under a fenced lease, so replicas cannot
  concurrently advance the same callback.
- Persists capped exponential retry deadlines (1s, 2s, …, capped at 30s)
  for transient failures: HTTP 5xx, network/timeout errors, and **HTTP
  408 / 425 / 429** (the codes a well-behaved server uses to ask for
  backoff).
- Treats other HTTP 4xx as terminal — the URL is permanently rejecting
  the payload and the retry loop stops.
- Bounds total in-flight HTTP deliveries at 64, so a misbehaving
  destination cannot starve the rest of the fleet.

Delivery is ordered and at least once. A response received immediately before a
worker crash can be repeated after the lease expires; each retry keeps the same
`X-Acteon-Delivery-Id` and `X-Acteon-Delivery-Sequence` headers so receivers can
deduplicate it. Later sequences remain blocked until the earlier sequence is
acknowledged or explicitly resolved by an operator.

Credential updates are fenced against an active delivery. After the current
sequence is acknowledged, an updated token or authentication credential applies
to the next claimed sequence. A restart reloads that configuration and the
acknowledged cursor from the configured state backend.

**Persistent DLQ.** Terminal failures (`4xx` other than `408/425/429`)
and exhausted-retry failures enter bounded dead-letter state inside the Task
envelope. List and inspect entries through the tenant operator view. Deleting
an entry explicitly discards that failed sequence and atomically allows the
next sequence to proceed:

During an upgrade, operator reads idempotently move append-only dead letters
from the pre-envelope key space into the Task envelope. Acteon deletes a legacy
row only after the Task CAS commits, so a crash can repeat the import without
losing the failure evidence.

Standalone callback configurations are mutable, so their upgrade uses a
coordinated cutover. Acteon retains private per-config migration provenance
during the mixed-version window. Native edits take ownership of their IDs, and
native deletes leave private bounded tombstones until cutover. After older
writers stop, the reconciler applies remaining legacy creates, updates, and
deletes in the Task CAS without resurrecting native deletes, then removes the
legacy rows.

Run that cutover against the same configuration file as the server. First stop
every pre-envelope Acteon binary, then generate a reviewable preview:

```bash
acteon-server -c acteon.toml a2a-push-cutover \
  --actor operator@example.com \
  --reason "move callback state into durable Task envelopes"
```

The JSON report lists Task and legacy-row counts, canonical row identities, and
content digests. It does not print callback tokens, authentication credentials,
DLQ payloads, or receiver errors. Apply only the digest from the reviewed
report:

```bash
acteon-server -c acteon.toml a2a-push-cutover \
  --actor operator@example.com \
  --reason "move callback state into durable Task envelopes" \
  --review-digest <review_digest>
```

Any inventory change rejects apply and requires a new preview. Successful apply
reconciles each Task atomically, removes legacy rows by exact state version, and
commits a backend-wide migration marker after an empty rescan. All later server
starts enforce that marker. A legacy callback row that reappears becomes an
operator-visible failure instead of being silently accepted.

```text
GET    /v1/a2a/push-dlq
GET    /v1/a2a/push-dlq/{entry_id}
DELETE /v1/a2a/push-dlq/{entry_id}
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
GET /a2a/.well-known/agent.json
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
| SSE connections | Server-configured per tenant | Shared `ConnectionRegistry` |

## Try it locally

The simulation example exercises the core Task lifecycle in-process, including
structured-input resolution and the authorization boundary, in ~200 ms:

```bash
cargo run -p acteon-simulation --example a2a_core_simulation
```

The output reads as a linear log with each scenario bracketed by a
banner. Engine actions and the `StreamEvent`s they emit line up by id.

## Federation status

The individual A2A agent service provides governed inbound execution within one
Acteon administrative domain. Its cross-domain boundary separates authority
admission from execution.
A configured peer first submits a signed delegation envelope to:

```text
POST /a2a/federation/envelopes:import
```

The peer must authenticate as the exact local principal assigned to its foreign
domain. Acteon verifies the audience-bound Ed25519 signature, current trust
revision, issuer epoch, freshness and expiry. It then requires the envelope's
recipient, skill, agent resource, binding digest, ingress effect and direct
effects to fit the service plan prepared from deployment configuration. A
successful response is a `FederationImportReceipt` with
`execution_started: false`: admission retains signed evidence and funds a
bounded local root, but it does not create an A2A task, register an effect or
call the recipient.

The peer can then submit one exact signed invocation and A2A message to the
imported agent:

```text
POST /a2a/federation/agents/{agent}/v1/message:send
```

The invocation signs the import ID and digest, replay nonce, complete typed A2A
message, issuance time and expiry. The request carries the same message as its
ordinary A2A body; Acteon requires semantic equality before accepting the task,
then independently derives and seals its local provider input. A foreign caller
does not need Acteon's private provider or action-type names. The route also binds
the authenticated peer to the import's issuer domain and trust ID, checks the
agent and current service binding, and reauthenticates immediately before any
new provider start. Trust, closures, target state, effect attenuation and root
budget are checked again inside the coordinator transaction that registers the
effect.

The same signed request is replay safe. Concurrent requests converge on one
task and one provider delivery. Exact completed results remain observable by
the configured peer after trust revocation without calling the provider again;
revocation blocks new work and unfinished delivery. Background recovery skips
federated acceptances because it has no peer credential.

Trust publication and terminal revocation are separate authenticated governance
operations:

```text
POST /v1/governance/federation/trusts
POST /v1/governance/federation/trusts/revoke
```

The caller needs an independently configured `can_manage_federation` grant and
an exact `federation_trust_ids` footprint. The server accepts only trust values
already present in the deployment-owned ceiling. Request data cannot introduce
a new key, domain, audience, recipient, effect or limit. Inspection returns only
the trust records inside that manager's footprint.

Acteon provides governed outbound delivery with signed capability-card
discovery and either a deployment-owned static bearer credential or a qualified
OAuth 2 client-credentials broker. Brokered access tokens rotate automatically,
stay only in process memory, and record only the provider revision in durable
evidence. The release suite starts two independent Acteon servers over HTTP,
negotiates the signed capability card, imports and invokes an agent, discards the
first invocation response after the destination commits, restarts both servers,
and explicitly replays the exact request. Redis-backed journals recover the same
remote task and prove one provider effect. Conformance evidence against a named
external implementation remains open. Peer credential state and effect authority
also reside in separate state
scopes: the route rechecks the peer just before dispatch, and the coordinator
separately performs the atomic execution check. A remote card, signature,
access token or cached envelope never overrides local permits, closures,
principal revocation or root limits.

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
| `-32003` | `PushNotificationNotSupported` | The published card does not enable push notifications. |
| `-32004` | `UnsupportedOperation` | The published interface does not support the requested operation. |
| `-32007` | `ExtendedAgentCardNotConfigured` | No extended card is configured for this tenant. |
| `-32009` | `VersionNotSupported` | The `A2A-Version` header names a version this server does not implement. |

REST responses translate these codes to HTTP statuses (`404`, `409`,
`400`, `500`, etc.) and put the message in the body.
A2A-specific JSON-RPC errors also carry the canonical `google.rpc.ErrorInfo`
reason and `a2a-protocol.org` domain in their `data` details array.

## Architecture references

- Design doc: `docs/design/a2a-protocol.md`
- Task engine: `crates/gateway/src/task_engine.rs`
- Protocol codec (JSON-RPC + REST): `crates/server/src/api/a2a.rs`
- Official JSON-RPC streaming and native SSE endpoint: `crates/server/src/api/a2a.rs`
- Discovery: `crates/server/src/api/a2a_discovery.rs`
- Push config CRUD: `crates/server/src/api/a2a_push.rs`
- Push delivery worker: `crates/server/src/api/a2a_push_worker.rs`
- Core types: `crates/core/src/bus_task.rs`, `bus_agent_card.rs`,
  `task_push.rs`
