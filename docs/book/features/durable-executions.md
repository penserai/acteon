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
(`PUT /v1/chains/definitions/{name}`). Every execution pins the definition
version it started with: the definition is stored once per
`{name}@{version}` in the state store (immutable, written on first use) and
every consumer — advancement, worker resume, reset, cancel notifications,
and the detail/history/DAG endpoints — resolves it from there. Deploying a
new chain version therefore never changes the behavior of in-flight
executions; new executions pick up the latest version. Deleting a
definition that other definitions still reference as a sub-chain is
rejected.

Old pinned versions are garbage-collected on the retention cadence
(hourly by default): a `{name}@{version}` entry is deleted only when no
execution — active, or terminal but not yet expired — still references it
*and* it is older than the registry's previous version for that name.
Executions sleeping for months keep their pinned definition for as long
as their state exists.

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

## Resetting executions

Any execution that reached a step — including terminal ones — can be reset
to re-run from that step:

```bash
curl -X POST "$ACTEON/v1/executions/$EXECUTION_ID/reset" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"namespace": "ns", "tenant": "t1", "step": "send-email", "reason": "SMTP outage resolved"}'
```

Step results from the reset point onward are discarded; results of steps
executed *before* the target on the recorded execution path are preserved,
so `{{steps.NAME.*}}` templates keep resolving. Any in-flight wait is
abandoned (a pending worker task is cancelled), an already-expired timeout
window is restarted, and the reset itself is recorded in the event history
(`execution_reset`).

## Concurrent updates and retention

Chain state updates use the version read with the execution. An operation delayed
past its lock lease cannot overwrite a newer cancellation, reset, or metadata
update; it returns a conflict error and callers can reload the execution. Retention
also checks the version atomically when deleting a terminal record, preserving
concurrent resets. The internal revision does not appear in HTTP responses.

External effects and auxiliary indexes remain separate operations. See
[chain state fencing](https://github.com/penserai/acteon/blob/main/docs/chain-state-fencing.md) and
[chain discovery recovery](https://github.com/penserai/acteon/blob/main/docs/chain-discovery-recovery.md) for upgrade
requirements, controlled race evidence, and the remaining recovery boundaries.

## Statuses

In addition to the existing chain statuses, durable executions introduce:

| Status | Meaning |
|---|---|
| `waiting_timer` | Paused on a durable timer |
| `waiting_signal` | Paused waiting for an external signal |
| `waiting_worker` | Paused waiting for an external worker task ([task queues](task-queues.md)) |
| `waiting_provider` | A governed provider attempt is in flight or requires reconciliation; the original receipt and budget charge are retained |

## Provider receipt recovery

Governed provider steps distinguish a completed result from work whose outcome
is still unknown. A parked execution reports `waiting_provider`; its
`wait_state.kind` is `provider`. The wait retains the logical step attempt,
provider call paths, execution IDs and receipt states. Polling observes these
same identities. It does not send the action again or reset its budget.

A completed receipt can repair the workflow result after expiry or revocation.
The next effect still needs current authority. Unresolved provider work remains
visible after the workflow deadline rather than being reported as a known
failure. Time passing is not proof that an external side effect did not occur.

Parallel groups preserve completed sibling results while unresolved branches
wait for evidence. Governed `any` and fail-fast groups observe all calls already
started in their current bounded batch before finalizing; later batches are not
started after a decisive result. A parallel timeout inspects retained receipts
without resending work. Cancellation of the workflow does not prove that an
already started external effect was cancelled or release its uncertain charge.

The Admin UI includes a provider-reconciliation filter and shows retained wait
details on chain executions. These receipt identities provide observation and
correlation; they are not execution credentials. This lifecycle applies to the
qualified provider chain profile. Sub-chain, deferred worker and cancellation-cleanup
authority adapters have separate qualification requirements.

### Cancellation of governed executions

Cancelling a governed chain first commits a permanent execution-instance fence
in the configured state backend, before publishing the cancelled chain status.
Every new provider start checks that fence through its signed budget ancestry.
A worker that loaded the chain or captured a child context before cancellation
therefore cannot start another effect after the fence commits. Other executions
of the same chain definition and principal continue under their own permits.
The fence uses retained signed root provenance, so expired execution authority
or a removed provider catalog entry does not prevent cancellation.

An effect registered before the cancellation fence may still complete.
Cancellation retains any provider wait already recorded in chain detail.
Attempt receipts and uncertain charges remain durable even when the worker had
not yet projected its wait into chain detail. It does not prove that an external service
stopped work. A provider that was already running can still persist its known
completion and settle the original charge. Cancelled chains do not advance
provider polling. Trusted host reconciliation can resolve retained attempts;
authenticated operator endpoints and remote probe adapters remain planned.
Starting fresh work requires a separate admitted execution. The existing
cancellation endpoint provides this behavior for the qualified provider-chain
profile; additional deferred-worker and cleanup adapters remain planned.

### Reconciliation from independent finality receipts

A governed provider host can install a `ProviderReconciliationVerifier` for an
exact provider binding. The built-in `HmacFinalityVerifier` checks locally signed
finality receipts from a separately trusted issuer. A receipt binds the original
context, actual provider action ID, registered attempt ID and nonce, retry ordinal,
and complete provider-binding digest. A request cannot select a verifier or key.

The accepted decisions are definitive completion with a provider response, or
`NoEffect` with a reason. A no-effect issuer must irreversibly fence the whole
attempt, including deferred work and possible future deliveries. An empty lookup,
an elapsed lease, or an agent's conclusion cannot establish that finality.
The provider adapter must qualify correlation with the actual external attempt;
Acteon does not infer it from matching input or action labels.

The host interface is:

```rust
let driver = driver.with_trusted_reconciliation_verifier(verifier)?;
let pending = driver.reconciliation_attempt(&context, &owner).await?;
// Obtain a finality receipt through the qualified external adapter.
let receipt = driver.reconcile(&context, &owner, &signed_receipt).await?;
let history = driver.reconciliation_record(&context, &owner).await?;
```

Verification does not call a provider or borrow the original execution permit.
A remote probe requires a separately qualified effect and current maintenance
authority. The first delivery is a Rust host integration; authenticated management
endpoints, configured provider-specific issuers and their SDK/UI controls are
subsequent adapters.

Acteon saves an immutable attestation before settling it through the same state
coordinator as starts and cancellation. The ledger retains the prior unresolved
state and original evidence reference, pins the new attestation's digest, and
releases concurrency once. Spent attempt units remain spent. Lost acknowledgements
repair from the retained proof; an uncommitted proof requires its original verifier
before it can be accepted. Accepted, digest-pinned history remains readable without
that verifier. A historical receipt reader also works without the provider
registry or original retry configuration.

A no-effect resolution completes the original operation with
`RECONCILED_NO_EFFECT`. It does not automatically retry it. An active parked chain
observes the original receipt, records its result under the same logical attempt,
and continues according to its chain policy. Every subsequent provider effect
still needs current admission. Cancelling a chain remains permanent even when
its retained provider work is later reconciled.

## Historical receipts

`HistoricalProviderStore` reads retained provider work through the configured
`StateStore`, the scope coordinator, retained context keys and the payload
encryptor when encryption is enabled. It requires no installed provider, current
retry configuration, clock or reconciliation verifier. Retiring a route does not
hide unresolved work or its accepted resolution.

The host must authorize the read and choose the permitted execution subject
from verified authentication and independent policy. Execution IDs and context references identify work; they do not grant
access. The Rust host interface supports lookup by either form:

```rust
use acteon_executor::governed::history::HistoricalProviderStore;

let history = HistoricalProviderStore::new(state, coordinator, contexts, encryptor);
let receipt = history.inspect_execution(execution_id, &owner).await?;
// A host that already holds the original context may use:
let receipt = history.inspect(&context, &owner).await?;
```

The projection includes the participant authenticated by the retained signed
context, recorded status, attempt history, original evidence
and outcome, accepted reconciliation, and an inherited cancellation fence. Its
observed authority generation identifies the control state seen with the read;
it is not an execution admission. Expiry and cancellation do not prevent reading
retained evidence, and reading does not renew permission to execute.

Only ledger-pinned evidence establishes an outcome. A known provider response
saved before ledger acknowledgement remains in-flight in this view. A saved but
uncommitted finality proof does not settle work or release concurrency. The reader
performs no writes, provider calls or repairs. Recovery and reconciliation remain
separate host operations. Missing or changed pinned records fail the read rather
than producing a replacement outcome.

`operation_integrity` distinguishes fully sealed operations, legacy records and
unstarted work. Delivery identity and retry limits appear in full-operation metadata only when
every registered attempt seals the complete operation.
Pinned result or finality evidence can authenticate historical binding metadata
for a legacy receipt even when its delivery identity and retry policy are
unsealed. Those fields remain separate from full-operation metadata. Legacy
receipts retain their narrower guarantees without invented seals. The
reader scans the bounded attempt protocol independently of unsealed retry
settings, so changing those settings cannot hide a later registered attempt.

Authenticated management clients can read this projection through
`GET /v1/governance/executions/{execution_id}` with `namespace` and `tenant`
query parameters. Access requires a current explicit `can_read_history` grant
covering the signed subject. A [history-only deployment](execution-permits.md#retire-execution-while-retaining-history)
can expose these reads after every live provider registration is removed.

## State backend and recovery contracts

Governed execution uses the configured `[state]` backend through `StateStore`.
The authority ledger, original operation, provider evidence, cancellation fence,
and reconciliation attestation share that backend. Redis-specific commands and
locks are not part of the execution protocol. Persistent deployments can select
Redis, PostgreSQL or DynamoDB; memory state is process-local and does not recover
after a server restart.

A fresh provider start pins the digest of the complete original operation in the
same coordinator compare-and-swap that admits and charges the attempt. This binds
the delivery identity and retry policy as well as the semantic input. Readers
reject a changed envelope, including when payload encryption is enabled. Older
records without an operation seal retain their existing evidence guarantees;
reading them does not create a new seal.

The shared multi-client provider contract covers one actual invocation, a second
worker observing in-flight work, cancellation, independent signed finality,
idempotent reconciliation, a late worker return and recovery without the original
verifier. It also checks that spent units remain spent and concurrency is released
once. The same contract has memory, Redis, PostgreSQL and DynamoDB test entry
points. CI explicitly runs the persistent-backend entry points against its test
services. These checks cover this provider protocol; other execution adapters
need their own backend and recovery contracts.

The qualified-plan handoff contract also runs through independent Redis,
PostgreSQL and DynamoDB clients in CI, including encrypted handoff recovery,
child provenance, shared-root charging and retained-definition integrity.

## See also

- [Task Queues](task-queues.md) — run chain steps on external workers
- [Workflows](workflows.md) — durable workflows as code
- [Task Chains](chains.md) — the underlying chain engine
