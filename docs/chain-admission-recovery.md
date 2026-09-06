# Chain admission recovery

This phase continues [chain discovery recovery](chain-discovery-recovery.md).
The chain row remains authoritative for progress, while worker-task and child
chain rows carry enough reverse identity to recover an admission interrupted
between records.

## Recovery contract

A worker step writes its `worker_task` before its parent can persist
`WaitingWorker`. On a later advance, the gateway scans the scoped task rows for
one task matching the chain ID, step index, step name, queue, action type, and
the current chain revision's timestamp. It adopts that task and preserves its
original timeout window. A settled adopted task wakes immediately so the normal
wait path consumes its stored result. Multiple matching records fail closed;
the gateway will not guess which external worker delivery is safe to reuse.

Sub-chain rows include the parent chain ID and step index. When the parent's
cached `child_chain_ids` list is absent after an interrupted parent write, the
gateway finds that reverse relation, reuses the existing child, and restores the
cached link on its next parent-state persist. It therefore does not create a
second child execution for the same sub-chain step.

Cancellation consults those reverse child relations in addition to the cached
list. Background chain reconciliation also finishes cancellation of an active
child whose parent is already cancelled, covering an interruption after the
parent terminal write and before its cascade completes.

Invalid worker or chain rows are retained for operator repair. They are not
used as an admission candidate. The matching relation includes namespace,
tenant, primary-row identity, and configured worker identity, so an unrelated
task or child cannot be adopted across scopes.

## Evidence

Manual-clock gateway contracts inject a parent chain CAS failure after a worker
task is durable and after a child primary row is durable. They verify that a
retry creates no second worker task and that cancellation reaches an unlinked
child. Existing chain discovery and fencing contracts continue to cover
ready-index repair and stale state writes.

## Remaining boundaries

This recovery covers internal worker and child admission only. Cancel
notifications, A2A projections, audit/history emission, and external effects
remain independent side effects. A sink that commits before reporting failure
still requires its own idempotency key; this phase does not establish
exactly-once delivery across a process crash, transport failure, or partition.
