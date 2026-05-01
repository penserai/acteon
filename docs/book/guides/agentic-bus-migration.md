# Migrating from `/v1/dispatch` + chains to the bus

> The agentic bus is **additive**. Nothing in your existing
> dispatch + chain pipeline has to move. This guide is for the
> cases where the bus model is a better fit and you want to
> refactor incrementally.

`/v1/dispatch` and chains were designed for *imperative* action
flows: a caller picks an action, the gateway evaluates rules,
the executor calls a provider, done. The bus is designed for
*conversational* flows: long-lived agents address each other by
identity, carry context across many turns, stream partial output,
and gate sensitive calls behind human approval.

Some workloads are unambiguously one or the other. Many sit in
the middle. This guide covers the typical "in the middle" cases
and the trade-offs of moving them.

## When *not* to migrate

Stay on dispatch + chains for any of:

- **Single-action provider calls** that don't need a conversation
  (send a Slack message, charge a card, open a PagerDuty
  incident). Dispatch is one round-trip; the bus would add a
  conversation, an envelope, and a result lookup for no benefit.
- **Tightly-defined multi-step pipelines** where the steps are
  all known up front and don't need to be addressable
  individually. Chains' sub-chains, parallel steps, and
  state-machine semantics are the right primitive.
- **Approval flows that gate dispatch on a chain step.** The
  existing `RequestApproval` rule path is integrated with the
  chain executor; Phase 6c approvals only gate bus tool-calls.

Reach for the bus when at least one of these is true:

- **Multiple long-running agents** need to talk to each other
  with stable identity.
- **The flow is open-ended** — a planner that decides, mid-loop,
  which tool to call next; a debate between agents.
- **Streaming partial output** — LLM tokens, progressive search
  results, partial JSON.
- **Replay is a first-class need** — you want to reconstruct
  what happened on a thread an hour ago without piecing together
  audit records.

## Pattern 1: fan-out dispatch → topic + subscriptions

### Before (dispatch + a downstream consumer pool)

```yaml
# A single action that needs to fan out to N consumers.
- action_type: "incident.created"
  payload:
    severity: critical
    service: payments
    summary: "5xx spike"
```

The gateway dispatches once. To fan out to multiple consumers,
you'd traditionally fork via chain steps or external glue.

### After (bus topic + subscriptions)

```python
from acteon_client import ActeonClient, CreateBusTopic, PublishBusMessage

client = ActeonClient("http://localhost:3000")

client.create_bus_topic(CreateBusTopic(
    name="incidents",
    namespace="alerting",
    tenant="acme",
    partitions=4,
    description="Incidents fanned out to all on-call consumers",
))

client.publish_bus_message(PublishBusMessage(
    topic="alerting.acme.incidents",
    payload={"severity": "critical", "service": "payments", "summary": "5xx spike"},
))
```

Each consumer subscribes via `POST /v1/bus/subscriptions` with a
unique `id` (which is also the Kafka consumer group id). They
each see every record exactly once — Kafka does the fan-out.

**What you gain:** consumer-side scale-out for free, lag
dashboards, DLQ routing for poison-pill messages, replay from
arbitrary offsets.

**What you pay:** an explicit topic + subscriptions to manage.
Use the [admin UI](../features/bus-phase-7.md) under
`/bus?tab=topics` to keep them visible.

## Pattern 2: tool-using agent loop → conversations + tool envelopes

### Before (dispatch + chain)

A planner agent that decides which tool to call next would,
under the dispatch model, dispatch a chain step per tool call,
threading results through `{{steps.NAME.body}}` templates. That
works when the chain is fixed; it bends quickly when the planner
itself gets to pick the next step at runtime.

### After (conversation + tool envelopes)

```python
from acteon_client import (
    ActeonClient, CreateBusConversation, PostBusToolCall,
    BusToolResultLookupParams,
)

client.create_bus_conversation(CreateBusConversation(
    conversation_id="planning-thread",
    namespace="agents",
    tenant="acme",
    participants=["planner-1", "calendar", "summarizer"],
))

# Planner posts a tool-call. The conversation replay primitive
# means it doesn't have to track the call itself — it can
# disconnect, restart, and rejoin from the cursor.
outcome = client.post_bus_tool_call("agents", "acme", "planning-thread",
    PostBusToolCall(
        call_id="call-1",
        tool="calendar.list_events",
        arguments={"day": "2026-04-29"},
        sender="planner-1",
    ))
receipt = outcome.produced  # not parked, since require_approval=False

# Wait for the matching result. The cursor on the receipt makes
# this race-free even on a busy events topic.
lookup = client.lookup_bus_tool_result("agents", "acme", "call-1",
    BusToolResultLookupParams(
        conversation_id="planning-thread",
        cursor=receipt.cursor,
        timeout_ms=5_000,
    ))
events = lookup.result.output["events"]
```

**What you gain:** stable agent identity, replayable thread,
typed envelopes, the [conversation drilldown](../features/bus-phase-7.md)
for operators to inspect mid-flight.

**What you pay:** an explicit conversation lifecycle. Open it
when the session starts, transition it to `closed` or `archived`
when done. The state machine catches stale-thread mistakes.

## Pattern 3: streaming output → stream envelopes

Dispatch is request/response. Chains are step/step. Neither
fits LLM token streaming.

```python
from acteon_client import ActeonClient, PostBusStreamChunk, PostBusStreamEnd

# Producer streams tokens as they arrive from the model.
for seq, tok in enumerate(model.stream(prompt)):
    client.post_bus_stream_chunk("agents", "acme", "thread-1",
        PostBusStreamChunk(
            stream_id="answer-1",
            chunk_seq=seq,
            body={"token": tok},
            sender="summarizer",
        ))

client.post_bus_stream_end("agents", "acme", "thread-1",
    PostBusStreamEnd(stream_id="answer-1", chunk_seq=seq + 1, status="complete"))

# Consumer plugs the SSE URL into its preferred SSE client.
url = client.bus_stream_consume_url("agents", "acme", "thread-1", "answer-1")
```

The SSE consume endpoint closes cleanly on the terminal
`stream_end` — no client-side bookkeeping needed.

## Pattern 4: high-risk action → HITL approval

### Before (`RequestApproval` rule on a dispatch action)

The existing approval system gates dispatch actions on a chain
step. It works well for chains but only for chains.

### After (`require_approval: true` on a tool-call)

```python
outcome = client.post_bus_tool_call("agents", "acme", "billing-thread",
    PostBusToolCall(
        call_id="refund-cust-7",
        tool="billing.refund",
        arguments={"customer": "cust-7", "usd": 42},
        sender="planner-1",
        require_approval=True,
        approval_reason="paid action — operator review required",
    ))

if outcome.was_parked:
    print(f"awaiting approval: {outcome.parked.approval_id}")
    # ...later, an operator approves via the admin UI or:
    # POST /v1/bus/approvals/agents/acme/<approval_id>/approve
    # The Kafka record then lands with an acteon.approval.id audit header.
```

**What you gain:** the approval row carries forward into the
produced Kafka record via the `acteon.approval.id` header, so
audit trails can correlate "who decided" with "what happened."
The [admin UI's approval queue](../features/bus-phase-7.md)
gives operators a one-click decide UX.

**What you pay:** the trust model is V1 — produce + state-row
update aren't atomic. The existing `RequestApproval` rule path
is integrated with the chain executor and offers stronger
guarantees for chain workloads. Use the bus path for new
conversational flows; keep the rule path for chain steps.

## Migration checklist

If you're moving an existing pipeline:

1. **Identify the conversation boundary.** What's the natural
   "thread"? An incident? A planning session? A user request?
   That's your `conversation_id`.

2. **List the participants up front.** Conversations with a
   non-empty `participants` list reject posts from outside the
   list. Open conversations (`participants: []`) accept any
   caller with the tenant grant — fine for V1 demos, not for
   multi-tenant production.

3. **Decide on schema binding.** If producers and consumers are
   on different teams, register a schema and bind it to the
   events topic. Phase 3's publish-edge validation catches drift
   before it lands on Kafka.

4. **Keep dispatch for what dispatch is good at.** A bus
   tool-call that fans out to a Slack notification doesn't
   replace `/v1/dispatch slack`. It's the right shape for the
   inter-agent message; the eventual side-effect is still a
   normal dispatch.

5. **Use replay during migration.** The conversation replay
   endpoint lets you reconstruct what happened during a refactor
   without re-running the workload.

## Don't migrate to the bus to "modernize"

If your pipeline is happy on dispatch + chains, it doesn't need
to move. The bus shipped because there were workloads dispatch
genuinely couldn't model — not as a replacement for what worked.
