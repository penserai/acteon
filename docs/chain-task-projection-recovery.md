# Chain-to-task projection recovery

Terminal chain state is authoritative. A linked A2A task is a separate record,
so a process can stop after a chain reaches `completed`, `failed`, `timed_out`,
or `cancelled` and before its task projection finishes.

`Gateway::reconcile_chain_task_projections()` scans retained terminal chain rows
with `task_id` set and reapplies their idempotent task projection. Background
cleanup runs this sweep automatically. Artifact IDs and the chain summary message
are stable, so a retry replaces existing artifacts and does not duplicate task
history. A task already in its projected terminal state is the durable receipt.

This recovers the chain-to-task transition and terminal result projection. It
does not make the two state stores transactional, recover a one-sided link, or
prove exactly-once delivery to an external A2A client.
