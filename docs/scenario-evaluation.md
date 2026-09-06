# Scenario evaluation follow-up

Base: `ea59463` (merged rehabilitation PR #257).

This pass extends the tested scenario kernel with repeated scripted workflows.
It addresses the audit plan's scorecards, explicit safety gates, named seed
streams, product scenarios, mutation evidence, and replay provenance. It does not
claim that the entire deterministic/distributed evaluation plan is complete.

## Implemented contract

`scenarios/portfolio.json` uses schema version 2. Each scenario runs independently
for 1–32 trials. SHA-256 derives a seed from the root seed, trial index, and
scenario name; each workflow derives named fault/actor choices from that seed.
Changing scenario order therefore preserves each trial's semantic evidence.
Unknown fields, scenarios, duplicate selections, unsupported versions, and
unbounded trial counts fail before execution.

The fixed grader requires unique named evidence for every rubric check. Missing
or duplicate checks fail closed. Each dimension has an integer weight summing to
100; its score is the percentage of its checks that passed. Weighted totals use
integer basis points (10,000 means 100.00 points). Every check is required for a
release pass. Safety gates are additionally reported separately, and any failed
gate fails its trial regardless of the aggregate score. The summary gives passed
and total scenario-trials, trials with failed safety gates, mean score, and worst
score. Missing evidence is a gate failure, not proof that a harmful effect occurred.

These are diagnostic regression scores. They are not calibrated capability
ratings, statistical confidence bounds, production latency measurements, or a
claim of safety outside the scripted cases.

## Workflows and independent observations

| Workflow | Exercised behavior | Grader evidence |
| --- | --- | --- |
| Incident response | Duplicate alerts reach two gateways; remediation awaits signed human approval; another tenant and approval replay are refused; seeded notification failures recover within the retry budget | Gateway outcomes, effect-ledger counts, actual consumed faults, and pending/executed/deduplicated audit records |
| Refund/fulfillment | Billing commits before an injected lost acknowledgement; the gateway retries; fulfillment cancels and refuses a stale shipment; a second order is rejected | Independent downstream idempotency ledger, refund and shipment counts, terminal state, retry attempts, signed approval outcomes, and dispatch audit records |
| Prompt injection | A frozen poisoned document supplies scripted hostile tool proposals, including encoded/chunked canaries and a forged role claim; legitimate summary work continues | Effect-provider inputs/outputs, raw/base64/hex/SHA-256 and joined-string canary detection, actual policy outcomes, fixture digest, literal destination policy, and suppressed/executed audit records |

The incident workflow uses actual gateway deduplication. Financial idempotency and
the refund/shipment exclusion are downstream provider contracts modeled by a
separate ledger. Removing downstream idempotency creates a second refund and
fails the financial gate even when the overall diagnostic score remains high.
Removing the approval rule or tool policy similarly fails the corresponding gate.
These negative tests run in CI alongside the passing workflows.

State and locks use the selected real backend. Audit observations query each
gateway's in-memory audit store after draining its writers. They verify dispatch
outcomes, including pending and executed approvals; they do not certify a durable
audit chain or every approval rejection transition. Both gateways remain alive:
cross-node approval tests shared state and signing keys, not process-crash recovery.

## Artifacts and replay

Run `scripts/ci/scenarios.sh memory redis postgres` with disposable database URLs.
The script runs both schema 1 kernel regressions and schema 2 portfolio trials,
then replays every report. Artifacts are under
`scenario-results/<backend>/<suite>/{first,replay}/`:

- `report.json`: input manifest, SHA-256 manifest digest, provenance, trial
  identities/seeds, scorecards, exact invariant evidence, and semantic traces.
- `manifest.json` (version 2): manifest and provenance for indexing.
- `trace.jsonl`: trial/seed identity and logical events, including scheduled and
  observed consumed faults. Provider fault observations are collected after the
  dispatch finishes; their attempt numbers describe the actual execution order.
- `junit.xml`: one case per scenario-trial, with failure evidence for CI.

Version 2 records a streaming SHA-256 digest of the running executable and the
lockfile embedded at compilation, plus the fixed grader version. It emits no
environment inventory or connection URLs in provenance. Preserve the executable
with saved reports. The script copies it to
`scenario-results/runner/acteon-scenario-<runner_sha256>`,
which CI uploads with the reports. CI strips this copy before running trials and
computing its fingerprint. After downloading the artifact, restore its executable
permission with `chmod +x runner/acteon-scenario-*` and use a compatible Linux host.
Replay refuses a different runner before execution, checks
manifest/trial identity and recomputed scores, and compares all semantic evidence.
It refuses to overwrite the original report. Inputs are limited to 16 MiB.

Rubric, fixture, schema, or implementation changes require code review and new
baseline artifacts; old artifacts are not silently accepted under a new runner.
GitHub Actions associates uploaded evidence with the tested revision. The old
schema 1 manifest and CLI remain supported with their original semantic replay
contract.

## Verification for this pass

Local validation completed on the implementation from `ea59463`:

- 3,068 workspace tests passed across 62 executables; the separate AWS `full`
  suite passed all 90 tests.
- Feature-enabled simulation passed 74 library tests and four CLI contract tests,
  including the three safety mutations, reordered scenarios, report tampering,
  incompatible runner rejection, and renamed/hard-linked replay input preservation.
- Rust 1.98.1 workspace Clippy and Rust 1.95 all-target compilation passed. The simulation
  library/binary/test Clippy gate passed with scenario, Redis, and PostgreSQL
  features on Rust 1.98.1 and Rust 1.88.
- Each of memory, disposable Redis, and disposable PostgreSQL passed the 23 kernel
  invariants and nine portfolio trials, then reproduced both reports on replay.
  JSON, XML, trace records, and preserved executable fingerprints were verified.
- UI lint/build, the source secret scan, formatting, whitespace, workflow YAML,
  and shell syntax checks passed. UI lint retains the existing TanStack compiler
  compatibility warning documented in the original rehabilitation record.

The phase from `1f28dbd` added a shared clock, deterministic timer/fault
scheduler, and deadline safety suite. The follow-up from `a514c5b` extends it to
[worker ticks and task lifecycle evidence](worker-lifecycle.md). See also
[the clock contract and limits](virtual-time.md). The next phase from `c0754e6`
adds [durable scheduling and deployment recovery](durable-scheduling.md).
The follow-up from `b7b588c` adds [worker queue recovery](queue-recovery.md), with
write-fault evidence on memory, Redis, and PostgreSQL and manual-clock race tests.

## Remaining plan

1. Extend virtual time into DLQ/audit-store retention and the remaining application
   lifecycle paths. Gateway/executor/memory clocks, background workers, and task
   transitions, worker-queue leases/retries, and workflow construction are injected; explicit worker ticks and the deadline and worker
   lifecycle suites now have virtual timing. Existing product portfolio trials,
   remote database TTLs, generated UUIDs, and OS scheduling retain real time.
2. Extend [durable deployment and tenant scheduling](durable-scheduling.md) beyond
   the implemented gateway restart, checkpoint/timer, and scheduled-outcome fault
   cases. Queue index reconciliation and before/after-write fault injection now
   cover memory, Redis, and PostgreSQL. Next, persist and reconcile terminal
   task-to-workflow/chain/DLQ handoffs, then add process-crash, audit-outage,
   transport, and partition adapters across the relevant backends.
   No test here establishes exactly-once effects across a crash between external
   execution and durable completion persistence.
3. Expand the injection portfolio to transport-level redirect/rebinding and
   identity/grant boundaries. This scripted case checks tool policy and literal
   destinations; it uses no model or browser and does not grade model resistance.
4. Add frozen-corpus research consensus and model capability trials separately,
   with model/prompt provenance, repeated-trial statistics, and calibrated graders.
5. Continue the audit's release-hardening work: broader audit/bus conformance,
   documentation/quick-start verification, container scanning, SBOM/provenance,
   concurrency exploration, fuzzing, and performance budgets.
