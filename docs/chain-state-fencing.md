# Chain state and retention fencing

This phase continues from merged main `12c9aa1` (PR #286). Chain state updates now
use the revision read with the execution, extending worker-result fencing to
normal advancement, timers/signals, worker and sub-chain waits, parallel steps,
retries, timeouts, cancellation, reset, and search attributes. A delayed operation
cannot overwrite a newer chain state after its execution lock expires.

## State contract

New chain and child-chain records use create-only insertion. Loaded chains carry
a runtime `state_version`; each successful update advances it so a single
operation can persist several transitions, including parallel execution setup and
completion. Existing records need no data migration. The revision is excluded
from JSON, stored payloads, and HTTP responses. Gateway execution reads reject
records whose chain
ID, namespace, or tenant disagrees with their key.

A conflicting update returns an error and leaves the winner intact. An update of
a deleted or expired record conflicts rather than recreating it. Callers should
reload the execution before deciding their next action. External provider work
may have occurred before a conflict; blindly repeating it is not an exactly-once
recovery strategy. Existing dispatch deduplication and interrupted-step behavior
remain in effect. Encryption applies to every chain write.

The existing A2A task-link writer already uses CAS and continues to do so. Worker
handoffs now use the same versioned chain reader and persistence helper as the
other gateway chain mutations.

## Retention contract

`StateStore::compare_and_delete(key, expected_version)` atomically removes only a
live record at that revision. It returns `false` for a missing, expired, or changed
record and preserves its value and expiry on conflict. Memory uses an atomic map
predicate, Redis uses Lua over both current hashes and legacy strings, PostgreSQL
uses a conditional `DELETE`, and DynamoDB uses a conditional delete request.

Chain retention re-reads the authoritative row after scanning, rechecks its status
and age, then conditionally deletes that revision. A concurrent reset or metadata
update invalidates the deletion decision. No separate read-then-delete fallback
is used. Existing retention policy enablement, age calculation, and compliance
hold behavior continue to apply. This does not change event/audit retention.

Custom Rust `StateStore` implementations and wrappers must implement or forward
the new atomic deletion method. Core `ChainState` struct literals must include
`state_version: None`; serialized data and HTTP/SDK request shapes are unchanged.
Stop old chain-writing binaries before upgrading: their unconditional writes do
not participate in the fencing contract. Execution IDs must not be reused for a
new execution after deletion; store versions are local to a record's lifetime.

## Evidence

Manual-clock gateway regressions cover timer arming, cancellation/reset races,
metadata updates, deletion during a pending write, delayed provider and parallel
results, version tracking through several parallel writes, encrypted state,
record scope, and retention racing a reset. Before the change, the timer and
metadata regressions reproduced a cancelled chain becoming active again.

The shared state-store conformance suite checks competing update/delete claims,
stale versions, missing and expired records, expiry preservation on conflict,
and tenant isolation. The same contract runs on all four state backends.

`scenarios/fencing.json` adds three seeded `chain_write_fencing` trials on memory,
Redis, and PostgreSQL. It uses each backend's real state and lock implementations.
Each explicitly armed stale-writer acquisition receives a 50 ms test lease; after the writer
reaches a controlled pause, the scenario waits 100 ms and lets a successor update
the execution. Production lease durations are unchanged. A separate paused
retention decision races an execution reset.

Grader `portfolio-v8` requires every dimension:

| Dimension | Weight | Observation |
| --- | ---: | --- |
| Execution state | 35 | Stale metadata/timer writers rejected, cancellation preserved, deleted receiver remains absent |
| Retention | 25 | Reset execution survives the stale deletion decision |
| Scope isolation | 15 | Mismatched receiver rejected without mutation |
| Encrypted state | 15 | Retained chain records remain encrypted |
| Observed faults | 10 | All three paused updates and the paused deletion were reached |

Mutations substitute an unconditional stale write, an unconditional stale delete,
or plaintext persistence; each fails a mandatory gate. Replay excludes generated
IDs, ciphertext, and wall-clock timestamps. These controlled tests do not kill
processes or establish model capability or production performance.

CI retains 21 report/replay pairs: nine memory suites and six each on Redis and
PostgreSQL, plus the exact executable used. Older reports require their preserved
runner.

## Verification

Local validation passed 3,139 workspace tests, including the nine new gateway
fencing regressions and all sixteen worker-handoff contracts from the preceding
phase, plus ninety AWS full-feature tests and 123 feature-enabled simulation
tests. State and lock suites on memory and disposable Redis, PostgreSQL, and
DynamoDB backends passed 39 tests in total,
including Redis legacy-string and shadow-key deletion coverage.

All 21 scenario report/replay pairs matched and passed every mandatory
gate. Their JUnit and JSONL artifacts and the preserved runner hash were checked.
The three unsafe fencing mutations each failed their intended gate. Validation
also passed workspace and feature-enabled simulation Clippy, Rust 1.88 checks,
the all-targets check, frontend lint/build, formatting, shell syntax, whitespace,
and source secret scanning.

Local DynamoDB tests used the system certificate bundle explicitly and ran test
cases serially after service readiness. Concurrent operations inside conformance
tests remained concurrent.

## Remaining boundaries

The version checks protect the authoritative chain record. Ready/pending indexes
are now rebuilt from it by [chain discovery recovery](chain-discovery-recovery.md),
including delayed terminal cleanup racing a reset. Signal buffers, child/task
admission, cancellation notifications, A2A projections, and audit/history writes
remain separate operations. They are not a transaction with the chain update.

Already-started external effects cannot be revoked by a state conflict. Child and
worker creation can still be interrupted between records. Process-crash,
transport/partition, audit-outage, and production-load evidence remain subsequent
work, alongside durable recovery for these remaining chain side effects.
