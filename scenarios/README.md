# Reproducible system scenarios

From the repository root:

```sh
scripts/ci/scenarios.sh memory
REDIS_URL=redis://127.0.0.1:6379 \
DATABASE_URL=postgres://acteon:password@127.0.0.1:5432/acteon_test \
  scripts/ci/scenarios.sh memory redis postgres
```

Use disposable external databases. PostgreSQL scenarios create unique table
prefixes and Redis scenarios create unique key prefixes. They never substitute
memory when an external service is unavailable. PostgreSQL tables are retained
until the disposable database is removed, allowing failure inspection.

The versioned JSON manifests select a backend, seed, and nonempty scenario list.
Unknown fields, versions, scenario names, and duplicate selections fail. Each run
emits `report.json`, `junit.xml`, and `trace.jsonl`. The script reruns each manifest
from the saved report and compares invariant results and semantic events. Failed
invariants, missing services, unsupported backend features, and replay divergence
produce a nonzero exit status. Preserve Cargo.lock and the code revision with
reports; seeded RNG output is not promised stable across dependency upgrades.

The script runs `rehabilitation.json` (schema 1, the six kernel cases below) and
`portfolio.json` (schema 2, three product workflows with three trials each), and
`queues.json` (schema 2, three queue recovery trials), and `handoffs.json`
(schema 2, three terminal-result recovery trials) on every backend.
Outputs are in `scenario-results/<backend>/<suite>/{first,replay}/`.

Run only the portfolio using the already-built executable:

```sh
target/debug/acteon-scenario --manifest scenarios/portfolio.json --output scenario-results/portfolio
target/debug/acteon-scenario --replay scenario-results/portfolio/report.json --output scenario-results/portfolio-replay
```

Schema 2 accepts `schema_version`, `seed`, `backend`, `trials` (1–32), and
`scenarios`: `incident_response`, `refund_fulfillment`, `prompt_injection`,
`queue_recovery`, `task_handoff_recovery`, `deadline_safety`, `worker_lifecycle`, or `durable_scheduling`. These last three
require memory.
It derives independent trial/scenario seeds, reports fixed weighted dimensions
and mandatory safety gates, and rejects missing/duplicate grading evidence.
Scores are regression diagnostics; every check must pass. Summary fields report
passed/total scenario-trials, `safety_gate_failures`, mean and worst scores in
basis points. They are not statistical estimates of model or production safety.

The portfolio tests duplicate alerts, approval across nodes, notification
outages, refund acknowledgement loss, rejected refunds, stale fulfillment, and
scripted exfiltration proposals from a frozen poisoned document. Negative tests
remove approval, downstream refund idempotency, or tool policy and require the
corresponding safety gates to fail. Canary values never appear in the report or
trace, including the intentionally failing injection test.

Schema 2 also writes `manifest.json` with runner and compiled lockfile SHA-256
fingerprints. Keep the executable with saved reports: replay refuses a different
runner, tampered identities/scores, or output that would overwrite its input.
The script preserves that executable at
`scenario-results/runner/acteon-scenario-<runner_sha256>`;
CI uploads it with the evidence. Downloaded CI binaries require a compatible Linux
host and may need `chmod +x` before use.
See [evaluation scope and remaining work](../docs/scenario-evaluation.md).

| Scenario | Boundary and independent observation |
| --- | --- |
| Generated policy | Swarm YAML generator → real YAML parser → gateway dispatch; blocked and pending actions produce no effect-provider call |
| Approval | Real gateway approval record and signature verification; another tenant cannot approve, another cluster node can approve, replay cannot execute again |
| Tenant deduplication | Multiple gateways share real state and locks; 16 redeliveries of a completed key are suppressed while another tenant's matching key executes |
| Retry recovery | Real action executor; seeded transient failures recover, exhaustion produces one DLQ entry |
| Evaluator integrity | Real evaluator subprocess and parser reject absent, nonfinite, ambiguous, and invalid scores |
| State failure | Unreadable rate-limit counters fail before provider execution; repairing the counter restores dispatch |

The separate `deadlines.json` suite uses a shared manual clock to test exact
dedup, approval, lease, and execution deadlines plus scheduled outage recovery.
It supports memory only and includes virtual elapsed time in its evidence.
See [clock injection and deadline evaluation](../docs/virtual-time.md).

The memory-only `workers.json` suite adds task timestamps, heartbeat/staleness
boundaries, audit/SSE emission, explicit group/timeout/scheduled ticks, and polling
cadence. CI runs all nine memory suites and replays them. Grader `portfolio-v8`
adds terminal handoff recovery to the existing rubrics; old reports still require
their preserved runner.
See [worker lifecycle evaluation](../docs/worker-lifecycle.md).

For the kernel and product portfolio, sequence numbers and parent links represent logical causality. The trace records
semantic outcomes and provider-call counts, excluding volatile UUIDs, durations,
and timestamps. Gateway wall-clock TTLs, external database clocks, and OS task
scheduling are not virtualized. These scenarios intentionally avoid timing races
as replay assertions. State/lock conformance tests separately cover leases, TTLs,
and concurrent ownership. The recorder offers seeded probabilistic failures and
injection through `SimulationHarness::start_with_providers` for additional tests.

These scenarios call production gateways directly, with recording providers at
the external-effect boundary. HTTP transport tests and shared SDK/Rust wire
fixtures exercise the network and serialization boundaries separately. They do
not demonstrate exactly-once external effects after a crash between provider
execution and completion persistence. Downstream idempotency and reconciliation
remain necessary for that failure window.

The memory-only `scheduling.json` suite exercises deployment restart with preserved
checkpoints, queue expiry/backoff, workflow timers, and leased one-shot scheduled
delivery. It injects a completion-write outage after an external effect and checks
rediscovery, stale receipt rejection, downstream idempotency, and tenant quota
boundaries. Three negative mutations must fail mandatory gates. See
[durable scheduling](../docs/durable-scheduling.md) for delivery, upgrade, and
remaining crash-window limits.

The all-backend `queues.json` suite repairs interrupted enqueue discovery after
gateway reconstruction, preserves retries after lost write acknowledgements,
and checks duplicate-ID ownership, queue/tenant scope, terminal cleanup, and
payload encryption. Three negative mutations must fail mandatory gates. Its
real-clock evidence is semantic; exact expiry and delayed-poll races use separate
manual-clock contracts. See [worker queue recovery](../docs/queue-recovery.md).

The all-backend `handoffs.json` suite covers lost terminal-write acknowledgement,
receiver outages, chain discovery repair, DLQ acknowledgement loss, scoped
receivers, and encrypted delivery progress. Negative mutations remove repair,
acknowledge an undelivered result, or remove downstream deduplication. See
[terminal handoff recovery](../docs/task-handoff-recovery.md) for the contract
and remaining evidence gaps. CI now retains 21 suite/backend replay pairs.


`fencing.json` exercises stale chain updates, deleted receivers, retention racing
reset, scope, and encryption on memory, Redis, and PostgreSQL. A test adapter
shortens explicitly armed acquisitions on the selected real lock backend; production leases
are unchanged. See [chain state fencing](../docs/chain-state-fencing.md) for the
conditional-delete contract, safety mutations, and remaining multi-record gaps.

`chain-recovery.json` rebuilds pending/ready discovery from authoritative chain
records after interrupted creation and signal delivery, and removes terminal
orphans. Its three mutations skip repair, retain an orphan, or write plaintext.
See [chain discovery recovery](../docs/chain-discovery-recovery.md).
