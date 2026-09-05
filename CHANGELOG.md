# Changelog

## Unreleased — rehabilitation

- Make one-shot scheduled delivery recoverable through durable CAS leases,
  renewal, outcome persistence, and discovery reconciliation. Clear old memory/Redis
  TTLs on CAS updates with no expiry, with shared backend conformance coverage. Execute due actions
  without re-scheduling; retain active records and encrypt all delivery state.
  Reject expired queue owners before reaping and replace caller-controlled quota
  exemptions with trusted dispatch origins. Add explicit queue/workflow clocks
  and replayable deployment/scheduling recovery with persistence-fault mutations.

- Extend shared clocks to background polling, worker decisions, and task
  mutations. Add explicit worker ticks, skip missed polls, and publish stale-task
  transitions to subscribers. Wire gateway-dependent workers independently of
  template sync. Add a replayable worker lifecycle evaluation with clock-failure
  mutations and exact boundary tests.

- Add shared gateway/executor/memory clocks, cancel-safe virtual timers, and a
  bounded deterministic scheduler with a replayable deadline safety suite.
  Reject approvals at exact expiry and memory lock acquisition after its wait
  deadline; prevent TTL eviction from deleting a replacement. Rule `now()` uses
  the evaluation time snapshot. Existing constructors keep production clocks.
- Add version 2 scenario evaluations with repeated seeded trials, weighted
  diagnostic scorecards, mandatory safety gates, exact runner fingerprints, and
  replay validation. Exercise incident approval/deduplication, refund ACK loss,
  and prompt-injection policy with scripted actors on memory/Redis/PostgreSQL.
  Mutation tests reject removed approval, downstream idempotency, and tool policy.
- Reject absent, malformed, ambiguous, nonfinite, and out-of-range evaluator
  evidence. Nonzero exit status and failing hard gates always fail acceptance.
  Challenge resolution requires independent, challenge-specific checks.
- Replace generated evaluation shell with reviewed program/argument plans.
  Add `acteon-swarm eval generate` and `eval run`; remove `generate_eval_script`.
- Recover in isolated Git worktrees, preserving existing changes, index, and
  stashes. Refuse promotion if the original workspace changed concurrently.
  Bound subprocess output and terminate owned Unix process groups on timeout or
  cancellation. Failed swarm runs now return a failing CLI status.
- Generate parser-compatible, tenant-scoped policies; require installation,
  reload verification, a notification provider, and an effective suppression
  probe before agents start. Hooks decode actual wire outcomes and fail closed.
- Instantiate real Redis/PostgreSQL state and locks in simulation; add injected
  providers, reproducible manifests, hard invariants, traces, JUnit, and replay.
  Honor isolated memory configuration and share approval signing keys across nodes.
- Make Redis counters honor set/CAS values and apply increments plus TTL atomically.
  Reject memory/DynamoDB counter overflow without changing the stored value.
  DynamoDB uses bounded conditional retries for counters, including legacy values.
  Extend shared conformance to cover invalid counters and value preservation.
- Make `DeadLetterSink::push` return `Result`. Encryption failures never store
  plaintext; unreadable ciphertext is retained and excluded from redelivery.
  Export `acteon_dlq_failures_total`. Rate-limit state errors fail closed.
- Introduce guarded webhook/A2A HTTP clients with connection-time destination
  checks, no proxy bypass, and constrained redirects. Custom webhook clients now
  use `GuardedClient`. Private/HTTP destinations require exact host exceptions.
- Require authentication or explicit development acknowledgement for nonlocal
  server binds. CORS defaults to same-origin. Compose uses a real configuration
  and publishes development ports on loopback.
- Fix metadata wire shape across Python, Node, Go, and Java using a shared Rust
  fixture; correct Python UTC serialization and typing. Restore clean Node
  installs/linting, package smoke checks, and Python wheel/type checks.
- Refresh vulnerable dependencies, migrate MCP/Azure SDK APIs, and move Wasmtime
  to its patched 36 LTS line with reduced compiled features. Add immutable CI
  action references, SDK/browser/backend/security checks, dependency inventory,
  Dependabot, and contribution/security documentation.

Read [rehabilitation verification](docs/rehabilitation.md),
[scenario limits](scenarios/README.md), and
[dependency decisions](docs/security-dependencies.md) before interpreting these
changes as deployment or correctness guarantees.
