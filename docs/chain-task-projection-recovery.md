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

## Executable evidence

`scenarios/chain-task-projection.json` runs on memory, Redis, and PostgreSQL.
It cancels a linked chain, lets the terminal chain write commit, and interrupts
the later task-row compare-and-swap after the projection has persisted its
artifact enrichment. A reconstructed gateway runs
`reconcile_chain_task_projections()` and must move the task from `working` to
`canceled`. A repeated sweep must return no work, while the terminal task
receipt remains stable. The mutation that skips the first sweep fails the
mandatory projection gate.

The adapter is a controlled write interruption, not an operating-system crash.
It exercises the durable boundary between the committed terminal chain row and
the task status write. One-sided links, external A2A delivery, transport
partitions, and a crash between an external effect and its durable receipt still
need separate idempotency and recovery evidence.
