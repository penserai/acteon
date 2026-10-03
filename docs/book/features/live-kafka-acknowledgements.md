# Live Kafka Acknowledgements

Use a long-lived acknowledged subscription when stream processing needs durable
input progress. Each record arrives with an opaque receipt tied to the consumer
session and its current partition assignment. Acknowledgements use that same
consumer; they do not join a temporary group member to commit raw offsets.

This is an `acteon-bus` library building block. Existing SSE subscriptions and
HTTP raw-offset acknowledgement endpoints keep their legacy behavior. A server
session registry and operations API are separate follow-ups. Backends without
this capability return `SubscriptionError::Unsupported`; Acteon does not
silently substitute an unfenced commit.

## Consume, checkpoint, acknowledge

```rust
use acteon_bus::{
    BusBackend, StartOffset, StreamCheckpointCoordinator, StreamCheckpointError,
    StreamOutboxEntry, SubscriptionCheckpointBatch, SubscriptionConfig,
};
use serde_json::{json, Value};

async fn process(
    backend: &dyn BusBackend,
    checkpoints: &mut StreamCheckpointCoordinator<Value, Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut subscription = backend.subscribe_acknowledged(
        "observability.acme.metrics", "detector-metrics", StartOffset::Earliest,
        SubscriptionConfig { max_in_flight: 256 },
    ).await?;

    loop {
        let delivery = subscription.recv().await?;
        // Apply domain processing to the recovered state. Include any newly
        // ready outputs in this same checkpoint, before acknowledging input.
        let state = json!({"last_payload": delivery.message.payload});
        let receipts = [delivery.receipt];
        let result = checkpoints.checkpoint_then_acknowledge(
            state, Vec::<StreamOutboxEntry<Value>>::new(),
            &mut [SubscriptionCheckpointBatch {
                source: "metrics",
                subscription: subscription.as_mut(),
                receipts: &receipts,
            }],
        ).await;

        match result {
            Ok(committed) => println!("generation {}", committed.checkpoint.generation()),
            Err(StreamCheckpointError::Acknowledgement { generation, error, .. }) => {
                // Some(generation) means persistence succeeded even though the
                // acknowledgement failed. Recover rather than regenerating outputs.
                return Err(format!("ack failed after generation {generation:?}: {error}").into());
            }
            Err(error) => return Err(error.into()),
        }
    }
}
```

For multi-source processing, keep one subscription per source and pass a batch
for each live session to `checkpoint_then_acknowledge`. Its source positions
are derived from receipts. Before persisting, it validates ownership and
requires receipts covering the complete commit prefix on each selected
partition, including earlier deliveries and any deferred acknowledgements that
would extend the commit. It then writes state, positions, and outputs atomically through the
checkpoint's CAS record, before attempting broker acknowledgements.

Kafka and the state store do not share a transaction. An assignment may be
revoked after checkpoint persistence; the error reports the durable generation.
Redeliveries must be checked against the recovered positions before domain
processing, and ready outputs must retain their original idempotency keys.
Records already represented by a restored checkpoint can be acknowledged
directly through their new live receipts without writing another checkpoint.
Checkpoint CAS prevents stale state versions from overwriting a winner. It
does not grant distributed ownership of an arbitrary multi-partition
aggregator: define the processor's partition cohort and handoff/reload protocol
before running multiple independent writers on one checkpoint.

## Ordering and ownership

- Receipts cannot be deserialized or reconstructed from `(partition, offset)`.
  A receipt from another session is rejected, including a replacement in the
  same consumer group.
- Each eager assignment/revocation advances a local epoch. Revocation clears
  ownership before Kafka unassigns partitions. Reassignment of the same
  partition does not restore an old receipt's validity. The epoch is not a
  broker generation ID.
- `acknowledge` validates the complete batch before marking anything processed.
  A later record can be acknowledged first, but its commit waits behind any
  earlier unprocessed delivery. Partitions progress independently. Broker log
  offset gaps from compaction or transactions are allowed; ordering follows
  records actually delivered.
- Successful commits return last-consumed offsets. Kafka receives the next
  offset (`last_consumed + 1`). Already committed receipts in the same epoch
  can be acknowledged again without another commit.
- Commit failures retain pending acknowledgement state for retry. Revoked or
  lost membership returns `StaleReceipt` and requires recovery under a current
  assignment. The broker also validates the original member's generation.

`ownership_changes()` returns a watch receiver with the current local epoch,
partitions, and closed state. Watch notifications coalesce and are served by
consumer polling; use them to inspect ownership, not to count rebalance events.

Acknowledgement operations run synchronous broker commits on blocking workers.
Their serialization guard survives cancellation of the calling future. A
cancelled call can still commit: retry the same receipts or inspect broker
progress rather than assuming the call did not happen. Dropping a session
closes its ledger and fences late local completion.

## Bounds and polling

`max_in_flight` defaults to 1,024 and accepts 1 through 100,000. It includes
processed records waiting behind gaps. `recv` returns `Capacity` before polling
another record when the bound is full. Finish the existing processing batch
and commit its prefix to release capacity. This bounds Acteon's delivered
receipt ledger; librdkafka has its own separately configured fetch buffers.

Keep calling `recv` within `max.poll.interval.ms`. There is no automatic
processing daemon or heartbeat/polling bypass for a wedged application. Budget
the complete processing/checkpoint/commit interval, and bound batch size and
latency. If the interval is exceeded, Kafka can remove the consumer and its
receipts must not advance input offsets. Acknowledgements fence input progress;
they cannot cancel or undo application effects that have already begun.

The live path forces auto-commit and auto-offset-store off, pins the requested
group ID, and uses classic eager `range` assignment. Pass-through settings
cannot override these correctness properties. Cooperative assignment needs
a separate per-partition epoch protocol and is not enabled by this API.

Malformed JSON and terminal consumption errors close the session. A later record
cannot be committed past an undispatched poison record. Recovery needs an
explicit data repair or dead-letter policy; the library does not silently skip
invalid input. Temporary broker connection failures continue reconnecting.

See [Stream Checkpoints](stream-checkpoints.md),
[Managed Stream Outbox](managed-stream-outbox.md), and
[Cascaded Neural Observability Detector](../guides/neural-observability-detector.md)
for the complete input-to-durable-dispatch path.
