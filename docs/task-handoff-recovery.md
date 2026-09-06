# Terminal worker-result handoff recovery

This phase continues from merged main `ea53c24` (PR #285). Completed or exhausted
worker tasks now retain durable delivery progress for their chain, workflow, and
configured DLQ destinations. A crash after saving the result cannot silently
discard those handoffs.

## Persistence and acknowledgement

The terminal result and `WorkerTask.handoff` are written in the same CAS. There
is no separate outbox insertion. Unfinished handoffs have no expiry, including
when the store commits the terminal write but its acknowledgement is lost.
Explicit completion/failure and lease exhaustion use this contract. Cancellation
of a queue task remains a companion operation for execution cancellation.

The gateway attempts delivery immediately. A failed destination leaves its flag
pending while independent destinations can finish. The HTTP completion response
confirms the worker result was accepted; downstream progress may still require a
cleanup retry. A lost HTTP response can be resolved by reading the task; retrying
completion still rejects a terminal task's old worker lease.

Each delivery pass claims a 60-second CAS lease and renews it every 20 seconds
while awaiting a destination. Only the unexpired owner can acknowledge or renew.
Cancellation of the delivering future leaves its claim recoverable at expiry.
Already acknowledged destinations are skipped. Even a lost final progress-write
acknowledgement can be finalized without repeating an acknowledged destination.
The task's normal 24-hour terminal TTL starts after all delivery flags are clear
and the claim is released. Handoff updates preserve the worker result timestamp.

DLQ attempts use one stable action ID stored with the handoff. Delivery is at
least once: a sink may commit before returning an error. A sink that requires
deduplication must persist that ID with its effect. The default in-memory DLQ
remains volatile and append-only; successful acceptance by it does not provide
durability across a server restart. Configure a durable sink when that retention
is required. Removing a required sink from a restarted gateway retains its
pending handoff instead of acknowledging it.

## Receiving execution recovery

Workflow writes use create-only insertion and versioned CAS updates. Worker-result
writes to chains also use the version read before processing the result. These
checks prevent delayed writes from overwriting a cancellation after an execution
lock expires. Existing task-ID/wait-state checks reject stale continuations.

Workflow continuation IDs are persisted before the task becomes discoverable.
Repair publishes the recorded ID without overwriting an existing task. Workflow
timers are reconstructed from the receiving execution's await state. If a chain
already saved its next step before ready-index publication failed, a retry repairs
that index from current state without applying the old step result again.

Terminal workflows persist `close_pending` before notifying their parent or
applying parent-close policies. Those records have no TTL until the close effects
succeed. Internal child-result signals persist a delivery receipt with their
checkpoint or buffered signal; retries cannot consume or buffer the result twice.
External signals retain their existing behavior. Cross-execution effects run
after releasing the local execution lock. The normal seven-day workflow TTL
begins after close effects finish. Finalization checks the delivered terminal
version, so an older timer or discovery pass cannot acknowledge a newer
cancellation's effects.

Stored payloads, results, delivery progress, and signal receipts use configured
payload encryption. HTTP DTOs omit internal handoff and receipt fields. The
workflow's runtime `state_version` is excluded from serialization entirely.

## Operation and upgrade

Enable background cleanup to run all three repair paths (disabled by default):

```toml
[background]
enabled = true
cleanup_interval_seconds = 60
```

Embedders without that processor must call
`Gateway::reconcile_worker_task_indexes()`, `Gateway::reconcile_worker_handoffs()`,
and `Gateway::reconcile_workflow_discovery()` after restart and periodically.
Attempt independent repairs even when one returns an error. A bad receiving
record or failed destination does not prevent other records from being tried.
The sweeps scan retained records and take execution locks; throughput and latency
under production load have not been characterized.

Stop old server/queue-consumer binaries before upgrading. They do not preserve
the new delivery metadata or participate in the workflow version checks. New
fields deserialize with defaults for old records. Legacy terminal tasks without
handoff metadata and legacy closed workflows without `close_pending` are not
automatically replayed: their prior effects cannot be inferred safely. Rust
embedders constructing core structs directly must accommodate the new fields;
existing constructors and HTTP/SDK request shapes continue to work.

Missing, unreadable, or mismatched receivers leave source results pending for
operator repair. Restoring a deleted/expired receiver requires its original
state; source results cannot reconstruct an entire execution. A receiver whose
effects finished can expire under its normal retention even if the source's final
acknowledgement was lost for that entire retention window. This may require
operator reconciliation of the retained source result.

## Evaluation and limits

`scenarios/handoffs.json` adds three seeded `task_handoff_recovery` trials on
memory, Redis, and PostgreSQL. Grader `portfolio-v6` adds mandatory dimensions:

| Dimension | Weight | Independent observations |
| --- | ---: | --- |
| Durable results | 35 | Lost terminal-write acknowledgement, workflow write outage, chain ready-index repair |
| Downstream delivery | 25 | Pending sink outage; lost sink acknowledgement produces one effect with downstream deduplication |
| Receiver isolation | 15 | Another tenant's workflow remains unchanged and the undelivered result is retained |
| Encrypted progress | 15 | Persisted worker, workflow, and chain records remain encrypted |
| Observed faults | 10 | Three store interruptions and both sink failures were consumed |

Skipping reconciliation, acknowledging an undelivered DLQ result, or removing
sink deduplication fails a mandatory gate. Trials reconstruct gateway objects
against the selected real backend, with a separate sink ledger. They do not kill
an OS process. Replay compares semantic observations and excludes generated IDs,
tokens, ciphertext, and wall-clock timestamps.

Manual-clock gateway contracts separately exercise exact claim expiry/renewal,
cancelled delivery futures, delayed writes after lock expiry, lost destination
acknowledgements, receiver timer/continuation recovery, independent destinations,
and retention across 25-hour sink and eight-day child-notification outages.
The write-fault adapter now also interrupts timeout and chain-ready indexing.

Run `scripts/ci/scenarios.sh memory redis postgres` with disposable database URLs.
CI runs seven memory suites and four on each remote backend, preserving fifteen
report/replay pairs and the exact executable. Old reports require their preserved
runner. These scripted cases do not assess model capability.

Initial multi-record admission, including child creation, is not an idempotent
transaction. Audit/history emission remains best-effort and may repeat or be
absent after a failure. Other chain paths retain their existing lock/write
behavior; the added chain CAS fencing applies to worker-result receipt. External
effects still require downstream idempotency. Process-crash, audit-outage,
transport/partition, and production-load evidence remain subsequent work.

## Verification

Validation for this phase includes the workspace test suite, all-targets build,
AWS full-feature tests, frontend lint/build, and current-Rust workspace Clippy.
Gateway and feature-enabled simulation Clippy also pass on Rust 1.88. Sixteen
handoff contracts cover the interruption and race cases above, alongside existing
gateway and simulation tests and the three failing safety mutations.

All fifteen suite/backend report/replay pairs pass against disposable memory,
Redis, and PostgreSQL fixtures, with 29 state/lock conformance tests passing.
Independent artifact validation confirms identical semantic reports, passing
safety gates, valid JUnit/JSONL, and the SHA-256 of the preserved runner. Formatting,
shell syntax, whitespace checks, and a source-only Gitleaks scan pass.
