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

## Evidence

The terminal-audit fault test takes the audit store offline after chain start,
cancels the chain, restores the store, and verifies that one recovery creates
the stable receipt while the next recovery is a no-op.

## Remaining boundary

Execution history is deliberately outside this recovery path. History allocates
its sequence separately and has no durable receipt keyed to the terminal chain
transition yet. A later phase must add that receipt before history can be safely
replayed after a crash. This phase also does not turn an audit store and chain
state store into one transaction or provide exactly-once external delivery.
