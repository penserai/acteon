# Durable scheduling and deployment recovery

This phase continues from merged main `c0754e6` (PR #283). It fixes one-shot
scheduled delivery and extends the shared clock into worker queues and workflow
construction, checkpoints, and execution-history timestamps.

## Delivery contract

The scheduled payload is authoritative. Active records and discovery entries
have no expiry TTL. Memory and Redis CAS now clear an old TTL when the caller supplies
`None`, matching PostgreSQL and DynamoDB; this also protects migrated records. The cleanup worker rebuilds missing discovery entries from
payload records; due polling keeps discovery until the consumer acknowledges an
outcome. A crash or closed channel after handoff leaves a recoverable record.

Each receipt carries a random ownership token stored by compare-and-swap in the
payload record. A lease expires at equality, after 60 seconds. The consumer uses
`Gateway::dispatch_scheduled_action(&receipt)`, which reloads the original action,
checks namespace/tenant/ID and ownership, atomically marks the receipt started,
and renews it every 20 seconds while dispatch is pending. Duplicate, completed,
expired, and replaced receipts return `Ok(None)`. A consumer that loses ownership
cannot acknowledge over its successor, even if its external request completes.

Successful dispatch persists its `ActionOutcome` before removing discovery.
The terminal record and outcome are retained for 24 hours. Here “successful
dispatch” means the gateway returned `Ok`: outcomes such as suppression and
provider failure are also terminal. Existing executor retry and DLQ behavior
still applies. A gateway error, cancellation, or failed outcome write leaves the
delivery eligible for retry after its latest lease expires.

The due action passes through current rules, silences, and guardrails again. A
matching Schedule verdict executes the action instead of recursively scheduling
it. Other verdicts retain their existing behavior. The consumer preserves the
payload, including arrays and scalar values. Quota is counted on initial
admission; trusted internal entry points select repeat-delivery accounting.
Caller payload fields `_scheduled_dispatch`, `_recurring_dispatch`, and
`_group_dispatch` no longer grant quota exemptions. The existing external
`_scheduled_dispatch=true` re-scheduling rejection remains for compatibility.

Encrypted records remain encrypted through claim, renewal, and completion. A
malformed, unreadable, or scope-mismatched record is retained for repair without
blocking other scheduled records. Discovery reconciliation runs on the
cleanup cadence and scans scheduled records, including retained terminal rows.

## Queue and workflow time

Queue leasing, heartbeat, settlement, cancellation, retry backoff, and chain
resume use the gateway clock. A heartbeat, completion, or failure at or after
lease expiry is refused even before a poll reclaims the task. The HTTP API maps
expired ownership to 409. Missing lease deadlines also fail closed.

`WorkerTask::new_at`, `WorkflowExecution::new_at`, and
`WorkflowExecution::record_checkpoint_at` allow explicit timestamps. Convenience
constructors retain real UTC. Server enqueue, workflow continuation, worker chain
steps, and checkpoint/history recording use their gateway's clock. Replayed
checkpoints preserve their original data and time. Supply the same clock to the
gateway, memory state/locks, and background processor for deterministic timing.

## Evaluation

```bash
cargo run --locked -p acteon-simulation --features swarm --bin acteon-scenario -- \
  --manifest scenarios/scheduling.json --output scenario-results/scheduling/first
target/debug/acteon-scenario \
  --replay scenario-results/scheduling/first/report.json \
  --output scenario-results/scheduling/replay
```

Schema 2 `durable_scheduling` uses three seeded trials with memory state and the
fixed manual epoch `2023-11-14T22:13:20Z`. Grader `portfolio-v4` adds these mandatory
safety dimensions:

| Dimension | Weight | Observations |
| --- | ---: | --- |
| Lease fencing | 20 | Expired heartbeat denied before reaping |
| Deployment recovery | 25 | Gateway restart, lease retry boundary, checkpoint preservation, tenant scope, continuation and timer timestamps |
| Durable discovery | 20 | Missing index repair; one injected outcome-CAS outage after an observed external effect; successful redelivery and terminal cleanup |
| Downstream idempotency | 20 | Two provider attempts produce one effect keyed by tenant and business object |
| Tenant isolation | 15 | Forged receipt denied, all three quota markers blocked, another tenant executes the same business object independently |

The deployment case reconstructs gateway and worker objects against retained
memory state. It does not terminate an OS process. A one-shot state-store adapter
fails exactly the scheduled completion write, after the provider ledger records
an effect. The replay records observed fault consumption, timestamps, ownership
outcomes, attempts, and effect counts, excluding random IDs and tokens.

Freezing the queue clock, disabling downstream idempotency, or skipping discovery
reconciliation fails the respective mandatory gate. These mutations run in CI.
The CLI rejects Redis/PostgreSQL for manual-time scenarios in both manifest
schemas and verifies combined deadline/worker/scheduling replay and clock
provenance. The CI script retains and replays the exact report-producing binary.

## Boundaries and upgrade

Delivery is at least once. An effect can succeed before its outcome is saved;
use downstream idempotency when repeating it is unacceptable. A delivery lease
protects state ownership and acknowledgement, not an external service from a
request already in flight. Retrying an initial scheduling request after an
ambiguous index-write failure can also create another schedule ID; creation is
not an idempotent transaction.

The worker accepts preceding records without embedded delivery fields and honors
outstanding legacy claim keys. Stop old scheduled consumers before enabling the
new ones: old binaries do not understand the embedded leases and completion
records. Existing payloads whose old TTL has already expired cannot be recovered.
Custom consumers must use the new receipt API; constructing a receipt now requires
`delivery_token`. Completed records remain visible in raw state until their TTL.

This phase does not make every queue/index mutation transactional. In particular,
crashes between queue-row transitions and pending/leased index updates still need
reconciliation work. Recurring schedules retain their existing handoff behavior.
Audit persistence, DLQ retention, real database TTLs, transport failures,
partitions, and OS scheduling remain separate evidence gaps. Remote backend runs
exercise the existing kernel and product suites, not virtual-clock lease expiry.
The scenarios contain scripted providers and make no model-capability claim.

## Verification

Local validation on 2026-09-05 passed 3,101 workspace tests, including 12 dedicated
gateway recovery contracts, plus 90 AWS full-feature tests and 120 feature-enabled
simulation tests (including five CLI contracts). Workspace all-target compilation
passed. Workspace Clippy passed on Rust 1.98.1; simulation feature and
gateway contract Clippy passed on that toolchain, and gateway/simulation library
and binary Clippy passed on Rust 1.88.

Twenty-six memory/Redis/PostgreSQL backend tests passed against disposable remote
services, including the shared expiry contract. Each backend passed 23 kernel
invariants and nine product-portfolio trials; memory additionally passed three
deadline, three worker, and three durable-scheduling trials. All nine suite/backend
report pairs matched their replays. JUnit, JSONL, and the preserved executable
fingerprint were independently verified. Disposable services were removed.

Frontend lint/build and source secret scanning passed. UI lint retains the
existing TanStack compiler compatibility warning. See the pull request for final
CI results.
