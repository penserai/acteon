# Clock injection and deadline evaluation

Base: `1f28dbd` (merged workflow evaluation PR #281).

This phase adds explicit virtual time for gateway dispatch, executor deadlines,
and memory state/locks, plus a bounded scheduler and a replayable deadline suite.
It is the first part of the clock/scheduler item in the rehabilitation plan.

## Clock contract

`acteon-time` provides a `Clock` trait with UTC time, monotonic elapsed time, and
cancel-safe timers. `SystemClock` is the default: production uses real UTC and
Tokio timers. `ManualClock` starts at a supplied UTC epoch and advances only when
its owner calls `advance_to`. UTC, TTLs, leases, and registered timers then move
together. Backwards or overflowing advances fail without changing the clock.
Dropping a timer removes its registration; due timers wake outside the clock lock.

Pass the same `Arc` clock to `GatewayBuilder::clock`,
`MemoryStateStore::with_clock`, and `MemoryDistributedLock::with_clock`. The gateway
shares it with its executor, circuit breakers, and default group manager. A custom
group manager must be constructed with that clock too. The clock is an embedding
API; there is no HTTP option for clients to change server time.

The gateway uses the clock for its rule context, approvals, quotas, silences,
calendar gating, dispatch audit timestamps/durations, and direct chain/workflow
decisions. The rule expression `now()` now uses the same evaluation snapshot as
the `now` identifier. Group creation, updates, flush readiness, and circuit
recovery use that clock as well. Default constructors remain available.

Execution deadlines are exclusive. If a timeout and provider completion are
both ready at the boundary, the timeout wins and the provider future is dropped.
Retry jitter remains configurable in production; the deadline suite uses a fixed
retry delay so its timing is reproducible.

## Boundary fixes

- Signed approval/rejection links expire at `now >= expires_at`. Previously,
  equality was accepted for the entire expiry second. The loaded record's own
  expiration is also checked, independently of backend TTL eviction. New signed
  links round fractional expiry up to a whole second while the precise record
  deadline remains authoritative, so links do not expire prematurely.
- A contended memory lock stops retrying at its wait deadline. Previously it
  attempted acquisition before checking whether its wait had already expired.
  Zero timeout still permits one immediate attempt. Polling sleeps are capped
  by the remaining wait time.
- Memory TTL reads evict an expired entry while holding the same map-entry lock
  used to inspect it, so eviction cannot delete a concurrently replaced value.

Approval expiration governs admission to a decision. This does not revoke an
already running external effect, undo an effect after a lost acknowledgement,
or establish exactly-once execution across a crash.

## Deterministic scheduler

`acteon_simulation::scheduler::DeterministicScheduler` polls one controlled root
future. On a timer wait, it advances the shared manual clock to the next timer or
declared external event. Simultaneous external events run in insertion order,
before polling work at that instant; an execution timeout still wins an exact
completion tie. The trace records every advance and applied event with exact
elapsed seconds/nanoseconds.

Every poll, advance, and event application consumes a step. The root future's
Tokio cooperative budget is disabled so prior work in the calling task cannot
change the virtual run; the scheduler enforces its own explicit budget. It cannot
preempt code that never returns from a poll. The input event count
is bounded by the same budget, and event IDs must be nonempty and unique. Past
events are rejected. A self-waking loop exhausts the budget; a pending future with
no virtual timer or declared event fails as an uncontrolled wait. Failures drop
the root future and cancel its timers. Unconsumed events remain inspectable.

This is a scheduler for controlled futures. It does not schedule operating-system
threads, sockets, database servers, or independently spawned tasks. Those need
explicit adapters before they can be used as deterministic evidence.

## Deadline suite

```bash
cargo run --locked -p acteon-simulation --features swarm --bin acteon-scenario -- \
  --manifest scenarios/deadlines.json --output scenario-results/deadlines/first

# Replay with the same executable.
target/debug/acteon-scenario \
  --replay scenario-results/deadlines/first/report.json \
  --output scenario-results/deadlines/replay
```

The schema 2 `deadline_safety` scenario uses the memory backend and fixed UTC epoch
`2023-11-14T22:13:20Z`. Remote backends are rejected before execution; Redis and
PostgreSQL retain their server clocks. Named seeds choose the declared outage
recovery time. The existing scorecard, safety-gate, artifact, and exact-runner
replay contracts apply. Grader version `portfolio-v2` adds this rubric:

| Dimension | Weight | Independent observation |
| --- | ---: | --- |
| Dedup expiry (safety) | 20 | Two gateways execute at 0 ms, deduplicate at 9,999 ms, execute at 10,000 ms; exactly two provider calls |
| Approval expiry (safety) | 25 | Approval at 1,999 ms succeeds; approval/rejection at 2,000 and 2,001 ms fail with no effect even when records have no TTL, with both whole-second and fractional creation times |
| Lease expiry (safety) | 20 | Exact-boundary expiry, denied stale renewal, stale release preserving a successor, and a waiter refused at its deadline |
| Execution deadline (safety) | 20 | 99 ms completion succeeds; 100 and 101 ms attempts time out at 100 ms without an effect or retained timer |
| Scheduled recovery | 15 | Declared outage/recovery consumed; provider attempts at 0, 100, 200 ms and one effect at 200 ms |

Mutation tests freeze only the approval clock or extend the execution timeout.
Each creates the forbidden behavior and fails its corresponding safety gate.
Additional tests cover timer cancellation, scheduler limits/event ordering,
counter/CAS expiry, rule time snapshots, group timestamps, circuit recovery,
report tampering, and CLI replay. Deadline reports replay byte for byte with the
same runner. CI runs the new suite and replay alongside the existing memory,
Redis, and PostgreSQL suites and preserves the executable in its artifacts.

## Verification

Local checks on 2026-09-05, from base `1f28dbd`:

- 3,081 workspace tests passed across 64 executables; the AWS `full` suite passed
  90 tests.
- 118 feature-enabled simulation tests passed: 79 library tests, 34 existing
  dispatch/rules/multi-node integration tests, and five CLI contracts. This
  includes deadline mutations, replay, and 512 virtual waits independent of
  Tokio's ambient cooperative budget.
- Workspace Clippy passed on Rust 1.98.1. Feature-enabled simulation Clippy
  passed on Rust 1.98.1 and Rust 1.88, including library/binary/test targets.
  Gateway clock contracts passed their dedicated Clippy check. Workspace
  all-target compilation, including examples, passed on Rust 1.95.
- Memory, disposable Redis, and disposable PostgreSQL each passed 23 kernel
  invariants and nine portfolio trials, with matching replays. Memory also
  passed three deadline trials and matching replay. Artifact JSON, JUnit, JSONL,
  and runner fingerprints were checked independently.
- Frontend lint/build, source secret scanning, Rust formatting, shell syntax,
  and whitespace checks passed. UI lint retains the existing TanStack compiler
  compatibility warning.

## Next work

Background polling workers, task-engine transitions, DLQ/audit-store retention,
external providers, remote database TTLs, and process/network scheduling still
have independent clocks or timers. Core convenience constructors and generated
UUIDs also retain their default timestamps unless a caller supplies/overrides
them. The deadline suite avoids those paths; it does not certify whole-system
virtual-time replay or durable audit behavior.

Next, inject worker/task lifecycle time and add explicit worker ticks. Then add
durable deployment and tenant scheduling scenarios with crash, lease, audit
outage, transport, and partition adapters. The wider remaining portfolio and
release work stays in [the evaluation plan](scenario-evaluation.md#remaining-plan).
