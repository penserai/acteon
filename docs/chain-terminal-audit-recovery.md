# Chain terminal-audit recovery

Terminal chain state commits before its summary audit record. An audit outage or
process stop in that interval used to leave a retained terminal chain with no
recoverable audit receipt.

Each terminal chain record now uses `chain-terminal-{chain_id}` as its stable
audit ID. `Gateway::reconcile_chain_terminal_audits()` scans retained completed,
failed, cancelled, and timed-out chains. If the audit store does not contain that
ID, it rebuilds the summary from the authoritative chain state. Background
cleanup runs the sweep automatically; embedded gateways can call it after restart
and periodically.

The audit ID is the durable receipt. A replay first reads that ID and writes only
when it is absent, so a successful retry is not duplicated by later sweeps. If
the audit store still cannot acknowledge the record, cleanup reports the error
and leaves the chain eligible for the next sweep.

The terminal state also preserves an explicit outcome when it differs from its
status. In particular, an interrupted `chain_definition_changed` failure now
replays with that outcome instead of being flattened to `chain_failed`.

## Evidence

The terminal-audit fault test takes the audit store offline after chain start,
cancels the chain, restores the store, and verifies that one recovery creates
the stable receipt while the next recovery is a no-op. The replayable
`chain-recovery` suite runs this contract against memory and Redis state, and
against PostgreSQL state plus a real PostgreSQL audit store. The outage injector
only wraps writes; successful recovery is persisted and read through the
selected audit backend.

## Remaining boundary

Terminal execution history now has its own durable receipt and recovery sweep.
Audit and history remain independent of the chain-state transaction, however:
an interruption before either receipt commits is reconstructed from the retained
terminal state. This phase does not provide a cross-store transaction or
exactly-once external delivery.
