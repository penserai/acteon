# Durable Executions

Task chains are durable executions: they can pause for hours or months on
timers, external signals, or worker tasks — consuming no resources while
waiting — and every state transition is recorded in an append-only event
history. Editing a chain definition never affects executions that are
already running.

## Execution event history

Every execution (chain or [workflow](workflows.md)) keeps an ordered event
log: when it started, each step's completion/failure/retry, timers started
and fired, signals awaited and received, and the terminal outcome.

```bash
curl "$ACTEON/v1/executions/$EXECUTION_ID/history?namespace=ns&tenant=t1" \
  -H "Authorization: Bearer $TOKEN"
```

```json
{
  "execution_id": "9b8f…",
  "events": [
    {"event_id": 1, "event_type": "execution_started", "name": "order-flow", "version": 3, "...": "…"},
    {"event_id": 2, "event_type": "step_completed", "step_name": "charge", "step_index": 0, "attempt": 1},
    {"event_id": 3, "event_type": "timer_started", "step_name": "cooling-off", "fire_at": "2026-06-12T00:00:00Z"},
    {"event_id": 4, "event_type": "timer_fired", "step_name": "cooling-off"},
    {"event_id": 5, "event_type": "execution_completed"}
  ]
}
```

Histories are capped at 5000 events per execution; once at the cap only
terminal events are still recorded.

## Durable timer steps

A `timer` step pauses the chain until the timer fires. Set exactly one of
`duration_seconds` (relative) or `until` (absolute):

```toml
[[chains.steps]]
name = "cooling-off"
timer = { duration_seconds = 259200 }   # sleep 3 days

[[chains.steps]]
name = "send-reminder"
provider = "email"
action_type = "send_email"
payload_template = { to = "{{origin.payload.email}}" }
```

While waiting the chain is in status `waiting_timer`; the background
processor wakes it when the timer fires. Timers survive restarts — they
live in the state store, not in memory.

## Wait-for-signal steps

A `wait_for_signal` step pauses the chain until an external signal is
delivered. The signal payload becomes the step's response body (available
to later steps as `{{prev.body.*}}` / `{{steps.NAME.body.*}}`).

```toml
[[chains.steps]]
name = "wait-approval"
wait_for_signal = { signal_name = "approved", timeout_seconds = 86400, on_timeout = "escalate" }
```

Deliver a signal:

```bash
curl -X POST "$ACTEON/v1/executions/$EXECUTION_ID/signal/approved" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"namespace": "ns", "tenant": "t1", "payload": {"approver": "renzo"}}'
```

Semantics:

- Signals delivered **before** the chain reaches the wait step are buffered
  durably (7-day TTL) and consumed immediately when the step is reached.
- `timeout_seconds` bounds the wait. On timeout, `on_timeout` names a step
  to jump to; without it the step fails and the step's `on_failure` policy
  applies (`abort` by default, `skip` to continue).
- Without a timeout the chain waits indefinitely (status `waiting_signal`).

## Definition versioning

Chain definitions carry a `version` that bumps on every update
(`PUT /v1/chains/definitions/{name}`). Every execution pins a full snapshot
of the definition it started with and advances against that snapshot — so
deploying a new chain version never changes the behavior of in-flight
executions. New executions pick up the latest version.

## Visibility & search attributes

`GET /v1/executions` lists executions across all chains — including
terminal ones — filtered by definition name, status, start-time window,
and **search attributes**:

```bash
curl "$ACTEON/v1/executions?namespace=ns&tenant=t1&status=waiting_signal&attr=team=payments" \
  -H "Authorization: Bearer $TOKEN"
```

Search attributes are seeded from the origin action's metadata labels and
can be updated mid-execution:

```bash
curl -X PUT "$ACTEON/v1/executions/$EXECUTION_ID/attributes" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"namespace": "ns", "tenant": "t1", "attributes": {"priority": "high"}}'
```

## Statuses

In addition to the existing chain statuses, durable executions introduce:

| Status | Meaning |
|---|---|
| `waiting_timer` | Paused on a durable timer |
| `waiting_signal` | Paused waiting for an external signal |
| `waiting_worker` | Paused waiting for an external worker task ([task queues](task-queues.md)) |

## See also

- [Task Queues](task-queues.md) — run chain steps on external workers
- [Workflows](workflows.md) — durable workflows as code
- [Task Chains](chains.md) — the underlying chain engine
