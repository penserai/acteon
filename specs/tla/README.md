# TLA+ Specifications for Acteon

Formal specifications of Acteon's concurrency-critical protocols, verified
using the TLC model checker.

## Why

Acteon is a distributed action gateway where multiple instances share state
through backends like Redis and PostgreSQL. The protocols for locking,
deduplication, circuit breaking, and chain execution involve subtle
interleavings that are difficult to test but possible to exhaustively verify
with TLA+.

See [docs/design/tla-plus-formal-verification.md](../../docs/design/tla-plus-formal-verification.md)
for the full research document.

## Specs

| Spec | File | Models | Verifies |
|------|------|--------|----------|
| **Dispatch Dedup** | `DispatchDedup.tla` | `gateway.rs` dispatch pipeline + dedup `check_and_set` | At most one execution per dedup key **within a dedup-TTL window**, even under dispatch-lock TTL expiry |
| **Circuit Breaker** | `CircuitBreaker.tla` | `core/circuit_breaker.rs` half-open probe slot | At most one *non-stale* probe in HalfOpen; no lock-loss wedge; valid transitions |
| **Hash Chain** | `HashChain.tla` | `audit/compliance.rs` `HashChainAuditStore` (the PR #227 max-tip fix) | The committed audit chain is contiguous — no duplicate sequence number, no fork — for every interleaving of concurrent writers |
| **Recurring Dispatch** | `RecurringDispatch.tla` | `background/workers/recurring.rs` claim + index re-arm (the PR #235 fix) | Each occurrence is dispatched **at most once**, even when a dispatch outlives the 60s claim TTL |
| **Message Bus** | `MessageBus.tla` | `gateway/group_manager.rs` `flush_group` notify-once | A grouped notification is emitted onto the bus **at most once** per window, despite concurrent flush workers / replicas |
| **Chain Ordering** | `ChainOrdering.tla` | `gateway.rs` `advance_chain` fresh re-read CAS (line 2986) | Each chain step is executed **at most once** and recorded **in contiguous order**, under concurrent `advance_chain` workers (isolates the idempotency-CAS layer) |
| **Approval Lifecycle** | `ApprovalLifecycle.tla` | `gateway.rs` `execute_approval` / `reject_approval` (the PR #225 two-phase) | An approval is decided once; the side-effect runs **at most once and only if approved**, only after the durable intent is recorded (intent-before-execute) |
| **Quota Counter** | `QuotaCounter.tla` | `gateway/quota_enforcement.rs` atomic check-and-increment + Block refund | **No counter drift** (no lost increment) and **no over-admission** past the limit, under concurrent dispatchers |
| **Bus Approval** | `BusApproval.tla` | `core/bus_approval.rs` + `api/bus.rs` Kafka pre-publish two-phase (Phase-10) | The parked envelope is published **at most once** and **only if approved** — `Approving` is committed before the produce; idempotent producer + reconciler |
| **Multi-Quota Rollback** | `MultiQuotaRollback.tla` | `gateway/quota_enforcement.rs` `enforce_quota_policies` block rollback | On a block, **every** counter touched is rolled back (all-or-nothing) — no partial leak on a non-blocking policy; no over-admit on any policy |
| **A2A Task Transition** | `A2aTaskTransition.tla` | `core/bus_task.rs` `can_transition_to` + `task_engine.rs` `cas_mutate` | Every committed task transition is **legal** and **terminal stays terminal**, under concurrent optimistic version-CAS (re-validates against the fresh row) |
| **Chain Cancel-Cascade** | `ChainCancelCascade.tla` | `gateway.rs` `cancel_chain` recursion + `advance_chain` guard | A cancel **cascades to every running descendant** (no orphan) and a **cancelled chain never resurrects** — relies on the recursion *and* the WaitingSubChain completion coupling |
| **Stream Replay** | `StreamReplay.tla` | `api/stream.rs` SSE reconnect (`Last-Event-ID` replay + live tail) | Across a reconnect, every event is delivered with **no gap and no duplicate** — relies on subscribe-before-replay *and* the `last_replayed_id` cursor dedup |
| **Retention Reaper** | `RetentionReaper.tla` | `workers/retention.rs` `run_retention_reaper` scan→delete | A record **held or not-expired at scan time is never deleted** — the `compliance_hold` skip and the expiry-check, each independently load-bearing (honest about the by-key-delete TOCTOU) |
| **DLQ Redelivery** | `DlqRedelivery.tla` | `executor/dlq.rs` `DeadLetterQueue` push/drain | Every DLQ entry is drained **exactly once and never lost** under concurrent push/drain — the `std::mem::take` take+clear atomicity |
| **Message Dedup** | `MessageDedup.tla` | `task_engine.rs` A2A message `check_and_set` + advisory probe | An A2A message is applied **at most once per dedup-TTL window** — the `check_and_set` is the gate; the read-only probe is advisory |
| **Key Rotation** | `KeyRotation.tla` | `crypto/lib.rs` `PayloadEncryptor` encrypt-active / decrypt-by-kid | **No value becomes undecryptable** — a key is never retired while a stored value is still stamped with it (the rotation contract decrypt-by-kid relies on) |
| **Ref-Graph Defense** | `RefGraphDefense.tla` | `task_engine.rs` `check_reference_graph` bounded walk | The reference-graph walk **terminates and rejects every graph-bomb** (cycle, over-depth, over-width) — cross-checked against an independent reachability oracle |

Each spec is self-contained: it inlines its own lock / state-store state machine
rather than sharing a module, so each can be model-checked independently.

Each spec is also adversarially validated: reverting the specific fix it models
(the max-tip read, the pre-dispatch re-arm, the flush mutex, the step re-read CAS,
the intent-before-flip ordering, the increment atomicity, the two-phase produce
ordering, the all-or-nothing rollback, the version-CAS re-validation, the cancel
recursion / completion coupling, the subscribe-before-replay / cursor dedup, the
reaper hold-skip / expiry-check, the drain take+clear atomicity, the dedup
check_and_set gate, the never-retire-a-live-key contract, the cycle / depth /
width / visited-filter checks) makes TLC report the corresponding safety
violation. The specs catch the real bug, not a tautology.

## Quick Start

### Prerequisites

- **Java 11+** on PATH (for TLC model checker)
- `tla2tools.jar` is downloaded automatically on first run

### Run all specs

```bash
cd specs/tla
make check-all
```

### Run a single spec

```bash
./ci/run-tlc.sh CircuitBreaker     # any one spec by name
./ci/run-tlc.sh HashChain
```

`run-tlc.sh` with no argument checks every `*.cfg` in the directory.

## CI Integration

Add to your CI pipeline:

```yaml
# GitHub Actions example
tla-specs:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions/setup-java@v4
      with:
        distribution: temurin
        java-version: 17
    - run: make -C specs/tla check-all
```

The CI configuration uses small model parameters (2 of each principal) for fast
feedback (all eighteen specs finish in a few seconds total). For nightly runs,
increase the constants in the `.cfg` files for deeper coverage.

## Project Structure

```
specs/tla/
  DispatchDedup.tla        # Spec: dispatch pipeline deduplication
  DispatchDedup.cfg        # TLC config (constants, invariants)
  CircuitBreaker.tla       # Spec: distributed circuit breaker half-open probe
  CircuitBreaker.cfg
  HashChain.tla            # Spec: compliance audit hash-chain sequencing (#227)
  HashChain.cfg
  RecurringDispatch.tla    # Spec: recurring at-most-once dispatch (#235)
  RecurringDispatch.cfg
  MessageBus.tla           # Spec: grouped-notification notify-once delivery
  MessageBus.cfg
  ChainOrdering.tla        # Spec: chain step at-most-once + in-order advance
  ChainOrdering.cfg
  ApprovalLifecycle.tla    # Spec: approval decided-once + intent-before-execute (#225)
  ApprovalLifecycle.cfg
  QuotaCounter.tla         # Spec: quota counter no-drift + no-over-admit
  QuotaCounter.cfg
  BusApproval.tla          # Spec: Kafka pre-publish approval, publish-once two-phase
  BusApproval.cfg
  MultiQuotaRollback.tla   # Spec: multi-policy quota all-or-nothing rollback on block
  MultiQuotaRollback.cfg
  A2aTaskTransition.tla    # Spec: A2A task version-CAS legal transitions
  A2aTaskTransition.cfg
  ChainCancelCascade.tla   # Spec: chain cancel-cascade, no orphan, no resurrection
  ChainCancelCascade.cfg
  StreamReplay.tla         # Spec: SSE reconnect replay, no gap, no duplicate
  StreamReplay.cfg
  RetentionReaper.tla      # Spec: reaper never deletes a held/live record
  RetentionReaper.cfg
  DlqRedelivery.tla        # Spec: DLQ drain exactly-once, no lost entry
  DlqRedelivery.cfg
  MessageDedup.tla         # Spec: A2A message at-most-once per dedup-TTL window
  MessageDedup.cfg
  KeyRotation.tla          # Spec: key rotation, no undecryptable value
  KeyRotation.cfg
  RefGraphDefense.tla      # Spec: reference-graph bounded walk, graph-bomb defense
  RefGraphDefense.cfg
  ci/
    run-tlc.sh             # CI runner (auto-discovers every *.cfg)
  Makefile                 # Convenience targets
  .gitignore               # Ignore downloaded tools and TLC output
```

## How to Read the Specs

Each `.tla` file follows this structure:

1. **CONSTANTS** — Parameters set in the `.cfg` file (number of gateways, thresholds, etc.)
2. **VARIABLES** — The system state (lock holders, store contents, process phases)
3. **Init** — Initial state of the system
4. **Actions** — Individual steps that processes can take (one per TLA+ action)
5. **Next** — Disjunction of all possible actions (the state machine)
6. **Safety properties** — Invariants that must hold in every reachable state
7. **Liveness properties** — Temporal formulas that must eventually be satisfied

The `.cfg` file tells TLC which invariants and properties to check.

## Modifying Specs

When changing Acteon's concurrency protocols:

1. Update the corresponding TLA+ spec first
2. Run `make check-all` to verify the new design
3. Implement the change in Rust
4. If TLC reports a violation, it prints the exact sequence of steps (trace)
   that leads to the bug — use this to fix the design

## Adding New Specs

1. Create `NewSpec.tla` and `NewSpec.cfg` in this directory
2. The CI script auto-discovers all `.cfg` files
3. Add a `make check-newspec` target to the Makefile
4. Update this README
