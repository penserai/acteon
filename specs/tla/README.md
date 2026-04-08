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

| Spec | File | Verifies |
|------|------|----------|
| **Circuit Breaker** | `CircuitBreaker.tla` | At most one probe in HalfOpen; valid state transitions |
| **Dispatch Dedup** | `DispatchDedup.tla` | At most one execution per dedup key, even under lock TTL expiry |

Shared modules in `common/`:
- `StateStore.tla` — Abstract model of the `StateStore` trait
- `DistributedLock.tla` — Abstract model of the `DistributedLock` trait

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
make check-circuit    # CircuitBreaker only
make check-dedup      # DispatchDedup only
```

### Direct invocation

```bash
./ci/run-tlc.sh CircuitBreaker
```

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

The CI configuration uses small model parameters (2 gateways, 2 actions)
for fast feedback (~30 seconds). For nightly runs, increase the constants
in `.cfg` files (3 gateways, 4 actions) for deeper coverage.

## Project Structure

```
specs/tla/
  CircuitBreaker.tla       # Spec: distributed circuit breaker
  CircuitBreaker.cfg       # TLC config (constants, invariants)
  DispatchDedup.tla        # Spec: dispatch pipeline deduplication
  DispatchDedup.cfg        # TLC config
  common/
    StateStore.tla         # Shared state store model
    DistributedLock.tla    # Shared distributed lock model
  ci/
    run-tlc.sh             # CI runner script
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
