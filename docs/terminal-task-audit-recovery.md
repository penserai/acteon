# Terminal A2A task-audit recovery

An A2A task's terminal state is stored independently from its audit record. An
audit outage or process stop after the task compare-and-swap commits previously
left a completed, failed, cancelled, or rejected task without a durable audit
receipt.

Terminal task transitions now use a deterministic audit ID derived from the
task namespace, tenant, and task ID. The ID is globally unique while remaining
stable across process restarts. Before recording a terminal transition, the
task engine checks that receipt; a lost acknowledgement therefore cannot create
a duplicate record.

`TaskEngine::reconcile_terminal_audits()` scans terminal task rows and restores
any missing receipt from the authoritative final state. The background cleanup
worker runs that sweep alongside the existing chain audit and history recovery
work. Recovery records the original terminal timestamp and final state. The
operation is marked `terminal_recovery` because the task row intentionally does
not retain a pre-terminal mutation transcript.

## Evidence

The task-engine outage test transitions a task to `cancelled` while its audit
backend rejects reads and writes. The terminal task state remains committed.
After the backend is restored, one reconciliation writes the stable receipt,
and a second sweep returns no work. Non-terminal task audit projections remain
best-effort; reconstructing every intermediate history, progress, and artifact
event would require a separate durable event receipt protocol.
