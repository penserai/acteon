# Agentic Bus — Phase 6c

> **Scope**: pre-publish HITL approvals for tool-calls. A requester
> opts in via `require_approval`; the envelope is parked under a
> `BusApproval` row in state instead of going straight to Kafka. An
> operator approves or rejects; only on approve does the record
> land. Streaming chunks and stream-end markers are out of scope —
> gating each token of a stream defeats the point of streaming. See
> the [master plan](../concepts/bus-master-plan.md).

Phase 6a and 6b made the bus protocol-aware (typed call/result
envelopes) and stream-aware (token-by-token chunks). Phase 6c adds
the missing safety primitive: the ability to require a human in the
loop *before* a sensitive tool-call reaches Kafka — without forking
the wire format or building a parallel pipeline.

## What ships in Phase 6c

| Surface | Shape |
|---|---|
| Core types | `acteon_core::BusApproval`, `acteon_core::BusApprovalEnvelope::ToolCall`, `acteon_core::BusApprovalStatus ∈ {Pending, Approved, Rejected, Expired}` |
| State key | `KeyKind::BusApproval` keyed by server-generated UUID v7 |
| HTTP | `POST /v1/bus/conversations/.../tool-calls` gains `require_approval` / `approval_reason` / `approval_ttl_ms`; `GET /v1/bus/approvals/{ns}/{t}`, `GET .../{id}`, `POST .../{id}/approve`, `POST .../{id}/reject` |
| Headers | Approved records gain `acteon.approval.id` so audit pipelines can correlate the produced Kafka record back to the row that gated it |
| Rust client | `post_bus_tool_call` returns `PostBusToolCallOutcome { Produced \| Parked }`; new `list_bus_approvals`, `get_bus_approval`, `approve_bus_approval`, `reject_bus_approval` |
| Tests | 12 core unit tests covering validation + serde + status transitions |
| Simulation | `bus_approval_simulation.rs` — parks two tool-calls, approves one, rejects the other, asserts the topic only contains the approved record |

## Model

### Park-then-produce

`POST /v1/bus/conversations/{ns}/{t}/{id}/tool-calls` with
`require_approval: true`:

1. Validates the envelope as it would for an immediate produce
   (envelope shape, sender ACL, schema binding, label caps).
2. Generates an approval id (UUID v7) and writes a
   `BusApproval { status: Pending }` row at
   `KeyKind::BusApproval:<ns>:<tenant>:<approval_id>`.
3. Returns `202 Accepted` with the approval id, `created_at`, and
   `expires_at`. **Nothing has been produced to Kafka.**

`POST /v1/bus/approvals/{ns}/{t}/{approval_id}/approve`:

1. Loads the row. If terminal (`Approved` / `Rejected` / `Expired`)
   → `409 Conflict`.
2. Soft-expire on read: if `expires_at < now`, return `409` with
   the expiry timestamp.
3. Re-resolves the conversation (so a topic that has rotated its
   `events_topic` since parking still publishes to the right place)
   and re-runs schema validation against the now-current binding.
4. Stamps the standard `acteon.envelope.kind = tool_call`,
   `acteon.tool.call_id`, `acteon.correlation_id`, `acteon.reply_to`
   headers — plus `acteon.approval.id` for audit correlation —
   and produces.
5. CAS-transitions the row to `Approved` with `decided_by`,
   `decided_at`, `decision_note`, and the produced
   `partition`/`offset`/`produced_at`.

`POST /v1/bus/approvals/{ns}/{t}/{approval_id}/reject`:

1. CAS-transitions the row from `Pending` to `Rejected` with the
   decision metadata. No Kafka record is ever produced for that
   `call_id`.

### Why park in state, not Kafka

The natural alternative — produce-then-tombstone — has a long lag
window during which the Kafka record is visible to consumers, plus
a tombstone-replication problem if the consumer doesn't honor it.
Parking in the state store keeps the contract simple:

- A `Pending` row never reaches Kafka. Consumers cannot observe
  it, full stop.
- The eventual produce only happens after a human decision.
- Audit trails capture both the approval row history and the
  resulting Kafka offset.

### TTL and expiry

Each parked approval carries `expires_at` (default 24h, max 7d).
V1 expires rows on read (the `approve` handler returns 409 if the
row is past its TTL; the list endpoint returns the row with its
stored `Pending` status — operators can filter visually using
`expires_at`). A future iteration can add a periodic reaper that
sweeps stale rows to `Expired`. The semantic outcome is the same;
the reaper just keeps the listing tidy.

## API shape

### Request approval

```http
POST /v1/bus/conversations/agents/demo/planning-thread/tool-calls
{
  "call_id": "call-pay-1",
  "tool": "billing.charge",
  "arguments": {"usd": 42},
  "sender": "planner-1",
  "require_approval": true,
  "approval_reason": "paid action — operator must approve",
  "approval_ttl_ms": 600000
}
→ 202 Accepted
{
  "approval_id": "019ddcea-be22-7781-ae90-4c496cb0d575",
  "namespace": "agents",
  "tenant": "demo",
  "conversation_id": "planning-thread",
  "correlation_token": "call-pay-1",
  "status": "pending",
  "created_at": "...",
  "expires_at": "..."
}
```

### List pending approvals

```http
GET /v1/bus/approvals/agents/demo?status=pending
→ 200 OK
{
  "approvals": [
    { "approval_id": "...", "status": "pending", "reason": "...", ... }
  ],
  "count": 1
}
```

### Approve

```http
POST /v1/bus/approvals/agents/demo/{approval_id}/approve
{
  "decided_by": "ops-1",
  "decision_note": "verified PO #4711"
}
→ 200 OK
{
  "approval": { "status": "approved", "produced_partition": 0, ... },
  "receipt": {
    "events_topic": "agents.demo.conversations-events",
    "call_id": "call-pay-1",
    "partition": 0,
    "offset": 18,
    "produced_at": "...",
    "cursor": "..."
  }
}
```

The `cursor` on the receipt is identical in shape to the Phase 5/6a
replay cursors — pass it to a follow-up `lookup_bus_tool_result` so
the result scan starts strictly after the approved record landed.

### Reject

```http
POST /v1/bus/approvals/agents/demo/{approval_id}/reject
{ "decided_by": "ops-1", "decision_note": "scope too broad" }
→ 200 OK
{ "approval": { "status": "rejected", ... }, "receipt": null }
```

## SDK example

```rust
use acteon_client::{
    ActeonClient, BusApprovalDecisionRequest, ListBusApprovalsParams,
    PostBusToolCall, PostBusToolCallOutcome,
};

let client = ActeonClient::new("http://localhost:3000");

let outcome = client.post_bus_tool_call("agents", "demo", "planning-thread",
    &PostBusToolCall {
        call_id: "call-pay-1".into(),
        tool: "billing.charge".into(),
        arguments: serde_json::json!({"usd": 42}),
        sender: Some("planner-1".into()),
        require_approval: true,
        approval_reason: Some("paid action".into()),
        ..Default::default()
    }).await?;

let approval_id = match outcome {
    PostBusToolCallOutcome::Parked(p) => p.approval_id,
    PostBusToolCallOutcome::Produced(_) => unreachable!(),
};

// ...operator UX...

let decision = client.approve_bus_approval("agents", "demo", &approval_id,
    &BusApprovalDecisionRequest {
        decided_by: "ops-1".into(),
        decision_note: Some("verified PO #4711".into()),
    }).await?;

let receipt = decision.receipt.expect("approve always returns a receipt");
assert_eq!(receipt.call_id, "call-pay-1");
```

## Authorization

- `require_approval` post still requires `BusOp::Publish` on the
  parking POST — parking is a privileged action; an unprivileged
  caller shouldn't be able to fill the approval queue.
- `list`, `get` require `BusOp::ManageConversation` (operator
  surface).
- `approve` requires both `ManageConversation` and `Publish` —
  approving *is* a produce, just gated.
- `reject` requires `ManageConversation` only.

Participant ACL on the originating conversation applies at park
time (the envelope's `sender` must be on the participants list when
the conversation is private). The approve handler does not re-check
participant ACL — the approval id is the trust token at that point,
and operators are by construction allowed to act outside the
participant set when servicing the approval queue.

## Trust model and limits (V1)

### Park-and-produce uses a two-step state machine

> **Updated in Phase 10.** Originally V1 (Phase 6c as shipped) made
> the failure modes invisible — a successful produce + failed CAS
> looked the same as "still pending." Phase 10 added the
> `Approving` intermediate state so the visibility gap is closed.

The flow:

```text
Pending ──approve──▶ Approving ──produce ok──▶ Approved
   │                     │
   │                     └──produce error──▶ (stays Approving;
   │                                          operator retries via
   │                                          a second `approve`)
   ├──reject────────────────────────────────▶ Rejected
   └──ttl elapsed──────────────────────────▶ Expired
```

What each branch means:

- **Pending → Approving.** The operator's first `approve` claims
  the row. `decided_by` / `decided_at` / `decision_note` are
  recorded. From this point the row is no longer rejectable.
- **Approving → Approved.** Set after a successful Kafka produce
  with the resulting `partition` / `offset` / `produced_at`.
- **Stuck Approving.** If the produce fails or the
  Approving → Approved CAS fails, the row stays in `Approving`.
  An operator (or admin UI) sees the row visibly stuck with the
  original `decided_by` recorded; calling `approve` again retries
  the produce *without* overwriting the audit metadata.

The non-atomicity isn't gone — Acteon doesn't run a Kafka
transactional producer in V1 — but the *visibility* gap is
closed. Operators can see exactly which rows are mid-flight, and
retry semantics preserve audit.

Trust model carry-overs:

- **Idempotent producer + consumer-side `call_id` dedup** keep
  the Kafka topic clean across retries. The lookup primitive
  returns the *first* record matching `call_id` (Phase 6a
  documented this), so even if the producer accidentally lands
  two records, downstream consumers see one.
- **Reject only works from `Pending`.** Once `Approving`, the
  operator has already decided "approve" and the produce is in
  flight; rejecting at that point would race the retry.
- **Background reconciliation worker** is a follow-up. V1 of
  Phase 10 ships the state machine + manual retry surface; an
  automatic reconciler that retries stuck `Approving` rows
  on a periodic sweep is the natural next step. The master
  plan tracks it as part of Phase 10.

A Kafka transactional producer + true outbox pattern is *also*
on the Phase 10 list — that closes the window completely (the
state-row update + Kafka produce happen in one atomic
transaction). The state-machine V1 is the foundation it builds
on.

### `require_approval` is per-call, not per-tenant policy

There's no server-side rule that *forces* a tool-call to require
approval. Phase 6c is a primitive — the requester opts in. A
follow-up phase can layer a policy engine that auto-flags certain
`tool` names or argument shapes by matching against rule
definitions, then injects `require_approval = true` server-side.

### Rejection is final

A `Rejected` row cannot transition back to `Pending` or to
`Approved`. The terminal-status check on both decision handlers
makes this explicit; double-decision races land on `409 Conflict`.

## What comes next

- **Phase 7** — UI: an approvals queue page, plus inline approve/reject from the conversation thread view.
- **Phase 8** — 5-SDK parity for the bus surface (Python, Node, Go, Java) including the new approval flow.
- **Future** — policy-driven auto-flagging of risky tool-calls; transactional producer + outbox for atomic park-then-produce.
