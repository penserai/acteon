# Memory Backend

The in-memory backend provides zero-dependency state storage and locking for single-process deployments.

<span class="badge development">Development</span>

## When to Use

- Local development
- Testing and CI
- Single-process deployments
- Prototyping

## Configuration

```toml title="acteon.toml"
[state]
backend = "memory"
```

No additional configuration is needed. This is the default backend.

## Usage

```rust
use acteon_state_memory::{MemoryStateStore, MemoryDistributedLock};

let state = Arc::new(MemoryStateStore::new());
let lock = Arc::new(MemoryDistributedLock::new());
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~50,000 ops/sec |
| **Latency** | < 1ms |
| **Persistence** | None (lost on restart) |
| **Distribution** | Single process only |
| **Mutual Exclusion** | Perfect (in-process mutex) |
| **Dependencies** | None |

## Limitations

- **No persistence** — all state is lost when the process stops
- **No distribution** — cannot be shared across multiple instances
- **No failover** — single point of failure

## Memory Audit Backend

For testing, there's also an in-memory audit backend:

```toml
[audit]
enabled = true
backend = "memory"
```

This stores audit records in memory with no persistence. Useful for tests and local development.
