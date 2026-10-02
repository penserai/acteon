# Stream Checkpoints and Outbox

`acteon-bus` provides a durable checkpoint coordinator for stream processors.
It stores three things in one optimistic-concurrency record:

- the processor's application state, such as an event-time window snapshot;
- the last fully represented offset for every source partition; and
- outputs waiting for idempotent downstream delivery.

The coordinator uses Acteon's `StateStore` contract. The same API therefore
works with the memory, Redis, PostgreSQL, and DynamoDB state backends.

## Why the outbox belongs in the checkpoint

Persisting window state and committing Kafka offsets are separate operations.
If the consumer commits first and crashes before saving its result, input is
lost. If it saves state but loses the ready output, the decision is lost.

`StreamCheckpointCoordinator` writes the state, source positions, and outputs
atomically before it exposes a broker commit plan. This gives the processor
at-least-once recovery without an XA transaction across Kafka and the state
store.

```text
consume → process → CAS(state + positions + outbox) → commit offsets
                                                       ↓
                         acknowledge output ← deliver with idempotency key
```

Delivery can repeat after a crash between the downstream call and the outbox
acknowledgement. The receiver must honor each output's `idempotency_key`.

## Initialize a processor

```rust
use std::sync::Arc;
use acteon_bus::{
    StreamCheckpointConfig, StreamCheckpointCoordinator,
    stream_checkpoint_key,
};
use acteon_state::StateStore;
use serde_json::json;

async fn open(
    store: Arc<dyn StateStore>,
) -> Result<StreamCheckpointCoordinator<serde_json::Value, serde_json::Value>, acteon_bus::StreamCheckpointError> {
    let key = stream_checkpoint_key("observability", "acme", "neural-detector");
    StreamCheckpointCoordinator::initialize(
        store,
        key,
        json!({"windows": []}),
        StreamCheckpointConfig::default(),
    ).await
}
```

Initialization loads and validates an existing snapshot or atomically creates
generation zero. A second processor instance can read the same checkpoint, but
only one writer can advance it: every update uses the `StateStore`
compare-and-swap version. A stale writer receives a typed conflict and can call
`reload()` before retrying its work.

## Checkpoint before committing offsets

Each position identifies the logical source, consumer group, topic, partition,
and last consumed offset. Outputs carry stable idempotency keys.

```rust
use acteon_bus::{
    StreamCheckpointCoordinator, StreamCheckpointError, StreamOutboxEntry,
    StreamPosition, StreamPositionLane,
};
use chrono::Utc;
use serde_json::json;

async fn save_batch(
    checkpoints: &mut StreamCheckpointCoordinator<serde_json::Value, serde_json::Value>,
) -> Result<(), StreamCheckpointError> {
    let position = StreamPosition {
        lane: StreamPositionLane {
            source: "metrics".into(),
            consumer_group: "detector-metrics".into(),
            topic: "observability.acme.metrics".into(),
            partition: 0,
        },
        offset: 418,
    };
    let output = StreamOutboxEntry {
        idempotency_key: "checkout-api:1790883720000".into(),
        created_at: Utc::now(),
        payload: json!({"route": "incident"}),
    };

    let persisted = checkpoints
        .checkpoint(json!({"windows": []}), [position], [output])
        .await?;

    // Only positions from `persisted` are now safe to commit to the broker.
    for position in persisted.positions() {
        // backend.commit_offset(...).await?;
    }
    Ok(())
}
```

`checkpoint_then_commit` packages the same order into one call. Its callback
runs only after the compare-and-swap succeeds. If a broker commit fails, the
error identifies the already-durable generation and position. Call
`persisted()` to retry that commit plan without producing another generation.
`checkpoint_then_commit_bus` commits directly through a `BusBackend`; for Kafka,
use it after draining and dropping the subscription stream as described in the
[subscription commit semantics](bus-phase-2.md#known-limitation-commit_offset-semantics).

Offsets may stay equal or advance. A regression is rejected before storage.
Positions and outputs are sorted deterministically in the snapshot.

## Deliver and acknowledge the outbox

Use [Managed Stream Outbox](managed-stream-outbox.md) for leased workers,
durable retries, dead letters, and metrics. Once a dispatcher is attached,
manual acknowledgements are rejected; completion must use its lease protocol.
The manual API below applies to checkpoints without a managed dispatcher.

Read `pending_outputs()` from the persisted plan or snapshot and deliver each
entry with its idempotency key. After successful delivery, remove the entries
in one new checkpoint generation:

```rust
async fn acknowledge(
    checkpoints: &mut acteon_bus::StreamCheckpointCoordinator<serde_json::Value, serde_json::Value>,
) -> Result<(), acteon_bus::StreamCheckpointError> {
    checkpoints
        .acknowledge_outputs(["checkout-api:1790883720000"])
        .await?;
    Ok(())
}
```

An unknown key is rejected, and a CAS conflict leaves the durable outbox
unchanged. Concurrent delivery may still call a downstream service more than
once, which is why the idempotency key remains part of the protocol.

## Bounds and operations

`StreamCheckpointConfig` sets hard limits for tracked source positions and
pending outputs. Invalid positions, empty IDs, duplicate output keys, capacity
violations, generation overflow, malformed snapshots, and stale writes fail
before the coordinator changes its local state.

Monitor checkpoint latency, CAS conflicts, pending-output count and age,
offset-commit failures, redelivery count, and the difference between consumed
and checkpointed offsets. Alert on an outbox that grows continuously: it means
processing is succeeding while downstream delivery is not.

For event-time correlation, store `EventTimeWindowSnapshot` as the coordinator's
state and use each emitted window's `idempotency_key()` for its outbox entry.
See [Event-Time Windows](event-time-windows.md) for that state machine.
