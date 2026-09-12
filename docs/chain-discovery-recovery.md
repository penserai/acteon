# Chain discovery recovery

This phase continues the chain-state fencing work from `3a260a2` (PR #287).
The chain record remains authoritative; `pending_chains` and the ready index are
rebuildable discovery records, not a source of execution truth.

## Recovery contract

`Gateway::reconcile_chain_discovery()` scans chain records across namespaces and
tenants. It restores the pending entry for every active chain and derives its
next ready time from persisted state:

- Running steps retain their configured delay or retry backoff.
- Timers and worker/signal waits retain their persisted deadlines and chain
  deadline.
- Buffered signals wake their waiting chain immediately.
- Sub-chain parents poll again after five seconds; interrupted parallel groups
  resume immediately from their durable progress state.

Terminal and absent chain records remove pending/ready orphans. A terminal
cleanup re-reads the primary row after deleting discovery. If a reset became
active while cleanup was delayed, discovery is restored before cleanup returns.
That closes the reset-versus-index-cleanup race without making indexes part of
the chain-state CAS transaction.

Background cleanup invokes this reconciliation whenever a gateway is attached.
Embedders without that processor should call it after restart and periodically.
Invalid or unreadable chain records are retained and reported as errors rather
than being deleted based on an unverifiable index.

## Evidence

Manual-clock gateway contracts cover an interrupted chain start, buffered signal
recovery after both discovery entries are lost, and an expired terminal-cleanup
lock racing a reset. The real-backend `chain-recovery.json` suite runs on memory,
Redis, and PostgreSQL. It checks interrupted creation, signal wake recovery,
terminal orphan pruning, encrypted primary state, observed write faults, and a
lost terminal-history acknowledgement. The latter commits the receipt to the
selected state backend before returning an injected error; reconciliation must
write exactly one original cancellation event from that receipt.

The same real-backend suite also makes the audit store unavailable after a
terminal cancellation commits. Once the audit store returns, reconciliation
must write the one stable, chain-derived cancellation audit record from the
authoritative terminal state. Its PostgreSQL run uses PostgreSQL for both the
selected state and audit stores, so the recovered receipt survives in the real
audit backend.

Grader `portfolio-v8` rejects all five controlled mutations: skipping discovery
recovery, retaining a terminal orphan, skipping terminal-audit recovery,
skipping terminal-history recovery, or persisting plaintext state. CI retains 28
report/replay pairs: thirteen memory suites and eight each on Redis and PostgreSQL.

## Verification

Local validation passed 3,142 workspace tests, 90 AWS full-feature tests, and
124 feature-enabled simulation tests. The 21 memory, Redis, and PostgreSQL
report/replay pairs passed all mandatory gates. Workspace and feature-enabled
simulation Clippy, all-target compilation, frontend lint/build, formatting,
whitespace, and scenario-script syntax also passed.

## Remaining boundaries

Recovery rebuilds only chain discovery. [Chain admission recovery](chain-admission-recovery.md)
now covers internal child and worker creation plus incomplete child cancellation
cascades. Signal buffers, cancellation notifications, A2A projections, and
terminal audit/history receipts each have independent recovery paths. External
effects remain independent: a crash between an external effect and completion
persistence still requires idempotent receivers and does not establish
exactly-once delivery. Process crashes, transport partitions, audit outages, and
production-load evidence remain subsequent work.
