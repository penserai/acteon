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

The versioned JSON manifest selects a backend, seed, and nonempty scenario list.
Unknown fields, versions, scenario names, and duplicate selections fail. Each run
emits `report.json`, `junit.xml`, and `trace.jsonl`. The script reruns each manifest
from the saved report and compares invariant results and semantic events. Failed
invariants, missing services, unsupported backend features, and replay divergence
produce a nonzero exit status. Preserve Cargo.lock and the code revision with
reports; seeded RNG output is not promised stable across dependency upgrades.

| Scenario | Boundary and independent observation |
| --- | --- |
| Generated policy | Swarm YAML generator → real YAML parser → gateway dispatch; blocked and pending actions produce no effect-provider call |
| Approval | Real gateway approval record and signature verification; another tenant cannot approve, another cluster node can approve, replay cannot execute again |
| Tenant deduplication | Multiple gateways share real state and locks; 16 redeliveries of a completed key are suppressed while another tenant's matching key executes |
| Retry recovery | Real action executor; seeded transient failures recover, exhaustion produces one DLQ entry |
| Evaluator integrity | Real evaluator subprocess and parser reject absent, nonfinite, ambiguous, and invalid scores |
| State failure | Unreadable rate-limit counters fail before provider execution; repairing the counter restores dispatch |

Sequence numbers and parent links represent logical causality. The trace records
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
