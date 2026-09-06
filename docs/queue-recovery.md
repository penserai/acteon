# Worker queue recovery

This phase continues from merged main `b7b588c` (PR #284). It repairs worker
discovery across interrupted writes, prevents duplicate enqueue from replacing
an existing owner, validates queue scope, and encrypts worker records.

## Recovery contract

The scoped `worker_task` row is authoritative. The existing `queue_pending`
index now discovers every active task, including leased tasks. Polling loads the
row and checks its namespace, tenant, ID, queue, status, and lease deadline.
Lease, heartbeat, retry, and expiry reclamation change only the row by CAS;
they never remove active discovery. A delayed poll cannot delete a newer owner's
retry entry. Legacy `queue_leased` values do not determine delivery or expiry.

Enqueue creates the row only if its ID is absent, then writes discovery. If the
second write fails, the row remains recoverable. Reconciliation reconstructs
active discovery from primary records and prunes orphaned, terminal, and
wrong-queue hints. Terminal index deletion is best-effort: a stale hint cannot
redeliver a completed, failed, or cancelled task. Corrupt, unreadable, or
scope-mismatched records are retained and logged for operator repair.

Enable server background processing to run reconciliation at the cleanup cadence
(60 seconds by default):

```toml
[background]
enabled = true
cleanup_interval_seconds = 60
```

Background processing is disabled by default. Embedders without an attached
background processor must call `Gateway::reconcile_worker_task_indexes()` after
restart and periodically. Expired leases are reclaimed lazily by queue polls,
using the row's deadline and the existing retry budget/backoff. Reconciliation
scans all worker rows and queue hints; polling reads all active entries even when
its delivery limit has been reached, so expiry reclamation continues. These are
linear scans, with no new throughput or latency guarantee.

Enqueue rejects noninitial task state, a zero attempt budget, and existing IDs.
Namespaces and tenants must be nonempty and contain no `:`; task IDs and queue
names use `[A-Za-z0-9._-]`. A duplicate ID cannot reset a live lease or retained
terminal result. Terminal rows retain their existing 24-hour TTL. The HTTP API
maps duplicate-ID errors to 409; it generates IDs itself, so this protection is
primarily relevant to embedded callers. HTTP retry after an ambiguous enqueue
failure can still create a second task. Creation is not an idempotent transaction.

With payload encryption configured, initial rows, lease changes, retries, and
terminal results remain encrypted. Legacy plaintext can be read and is encrypted
on the next row mutation. Queue index values contain no payload. This is
encryption at rest; authorized worker API responses still contain their payloads.

## Redis compatibility

Create-only entries now use versioned hashes. Reads atomically promote legacy
strings while preserving their remaining TTL. Reads and successful CAS remove
shadow strings left by older implementations, preventing the original value
from reappearing when a newer hash expires. CAS conflicts preserve the current
value and TTL; a missing record always conflicts, including expected version 0.

The shared expiry contract also exposed a DynamoDB CAS gap: an expired item
retained by its asynchronous TTL sweep could be updated back into live state.
The conditional write now checks expiry together with the expected version.
Create-only replacement of an expired item is also a single conditional put;
it no longer deletes a row after a separate expiry read, which could erase a
concurrent replacement. A 32-caller regression checks that exactly one wins.

Stop old queue consumers/server binaries before enabling the new ones: old poll
code still deletes pending discovery during leasing. Run reconciliation after
the upgrade, before resuming workers. Already-expired primary rows cannot be
reconstructed from index hints. An old Redis shadow that has already outlived its
authoritative hash has no tombstone identifying it and cannot be distinguished
from a legitimate legacy value; inspect such records during migration.

## Evidence and limits

`scenarios/queues.json` runs three seeded `queue_recovery` trials on each of
memory, Redis, and PostgreSQL. The shared one-shot write adapter injects failures
before a write or after a successful commit, and can pause operations for
controlled interleavings. Each run reconstructs gateway objects over retained
state; it does not kill an OS process.

Grader `portfolio-v5` adds these mandatory dimensions:

| Dimension | Weight | Observations |
| --- | ---: | --- |
| Queue discovery | 35 | Interrupted enqueue repair, lost retry acknowledgement, legacy-index repair |
| Ownership and scope | 25 | Duplicate enqueue preserves owner; wrong queue and tenant cannot acquire a pending task |
| Terminal cleanup | 20 | Cleanup failure retains result, later sweep removes hints, no redelivery |
| Payload encryption | 10 | Both tenants' persisted records remain encrypted |
| Observed faults | 10 | All three requested write interruptions were consumed |

Skipping reconciliation, deleting retry discovery, and disabling encryption each
fail a mandatory gate. Backend trials use real clocks and compare semantic
observations, excluding UUIDs, tokens, ciphertext, and wall-clock timestamps.
Manual-clock gateway contracts separately test exact expiry/backoff boundaries,
lost reclaim acknowledgements, concurrent enqueue/poll, and a delayed old poll
resuming after reclamation.

Run and replay all suites with disposable database URLs:

```sh
scripts/ci/scenarios.sh memory redis postgres
```

CI runs six memory suites and three suites on each remote backend, retaining
twelve report/replay pairs and the exact executable. Old reports require their
preserved runner. These scripted cases do not measure model capability.

Recovery here covers task discovery and ownership. A crash after terminal task
persistence but before workflow/chain continuation or DLQ handoff can still lose
that handoff; it needs a durable outbox/reconciler in the next phase. External
effects remain at least once and require downstream idempotency where duplicates
are unacceptable. Audit outages, transport failures, partitions, and production
load remain separate evidence gaps.

## Verification

Local validation on 2026-09-05 passed 3,113 workspace tests and the final 13-case
queue recovery target, plus 90 AWS full-feature and 121 feature-enabled simulation
tests. All-target compilation, current-stable workspace/feature/test Clippy, and
Rust 1.88 gateway/simulation library/binary Clippy passed.

Twenty-nine memory/Redis/PostgreSQL backend tests and nine DynamoDB Local tests
passed against disposable services. All twelve suite/backend report pairs matched
their replays; JSON gates, JUnit, JSONL, and the preserved executable fingerprint
were independently verified. The services were removed after testing.

Frontend lint/build, source secret scanning, formatting, whitespace, and shell
syntax checks passed. UI lint retains the existing TanStack compiler compatibility
warning. See the pull request for final CI results.
