# Chain cancellation notification recovery

This phase continues [chain admission recovery](chain-admission-recovery.md).
Cancellation notifications are now a durable handoff rather than an untracked
dispatch after the terminal chain write.

## Recovery contract

The terminal `Cancelled` state and its notification handoff are written in one
compare-and-swap. The handoff contains the target selected from the execution's
pinned definition (or the existing webhook fallback when it is unavailable), a
stable action ID, an acknowledgement timestamp, and a 60-second delivery lease.
A cancellation that reaches durable state therefore cannot lose its notification
merely because the process stops before dispatch.

Cancellation attempts delivery immediately after it releases the chain lock.
`Gateway::reconcile_chain_cancellation_handoffs()` scans retained chain rows and
retries incomplete handoffs; `BackgroundProcessor` calls that sweep during its
normal cleanup. An embedded gateway without that processor must call the sweep
after restart and periodically.

Only the owner of an unexpired lease can acknowledge or release a handoff. A
successful acknowledgement marks it complete in the chain row. A provider
failure releases the lease for the next sweep. A crash after provider execution
and before acknowledgement leaves the lease until expiry, then replays the
same action ID. Downstream systems must use that ID for idempotency: the gateway
provides at-least-once delivery across this ambiguity, not exactly-once external
effects.

Legacy cancelled rows without handoff metadata are retained but never replayed,
because the old notification target and whether it already ran cannot be
reconstructed safely.

## Evidence

Manual-clock fault tests cover a provider outage after the terminal write and a
failed acknowledgement write after provider success. They verify that cleanup
completes the handoff and that every retry uses the persisted delivery ID.

## Remaining boundaries

A2A task projections, audit/history emission, and any provider-side effects
outside this notification are still separate operations. Transport partitions,
production backend fault runs, and downstream idempotency verification remain
required before treating a cancellation as an exactly-once workflow.
