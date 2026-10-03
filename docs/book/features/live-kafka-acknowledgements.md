# Live Kafka Acknowledgements

Use a long-lived acknowledged subscription when stream processing needs durable
input progress. Each record arrives with an opaque receipt tied to the consumer
session and its current partition assignment. Acknowledgements use that same
consumer; they do not join a temporary group member to commit raw offsets.

This building block is available through `acteon-bus` and the HTTP session API.
Existing SSE subscriptions and raw-offset acknowledgements keep their legacy
behavior for subscriptions without receipt-required policy. Backends without
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
Each session subscribes to one literal topic; Kafka regex subscriptions are
rejected so a receipt's topic always identifies the concrete source lane.

Malformed JSON and terminal consumption errors close the session. A later record
cannot be committed past an undispatched poison record. Recovery needs an
explicit data repair or dead-letter policy; the library does not silently skip
invalid input. Temporary broker connection failures continue reconnecting.

See [Stream Checkpoints](stream-checkpoints.md),
[Managed Stream Outbox](managed-stream-outbox.md), and
[Cascaded Neural Observability Detector](../guides/neural-observability-detector.md)
for the complete input-to-durable-dispatch path.

## HTTP subscription sessions

The server exposes the same live-consumer capabilities over HTTP when built
with `--features bus`. Create a durable subscription with `receipt_required:
true` and `ack_mode: "manual"`. Its `consumer_group` is derived from namespace,
tenant, and subscription ID. Legacy subscriptions keep their existing group;
enabling this policy on a new subscription creates a separate group and does
not migrate old broker offsets. The `acteon-live-` group prefix is reserved:
legacy SSE cannot join it, and the raw-offset acknowledgement endpoint rejects
receipt-required subscriptions on every server instance.

All paths below start with
`/v1/bus/subscriptions/{namespace}/{tenant}/{id}/sessions`:

| Method and suffix | Request | Result |
|---|---|---|
| `POST` | `{"request_id":"<client UUID>"}` | Open a server-owned consumer; return `session_id` and `consumer_group` |
| `GET /{session_id}` | — | Phase, epoch, partitions, pending count, buffered bytes, closure reason |
| `POST /{session_id}/receive` | `{"max_messages":64,"wait_ms":10000}` | Bounded batch containing opaque `receipt_id`, message, position, and epoch |
| `POST /{session_id}/validate` | `{"receipt_ids":["<receipt UUID>"]}` | Complete-prefix validation and broker-derived checkpoint positions; no commit |
| `POST /{session_id}/ack` | Same receipt-ID body | Revalidate the prefix and acknowledge through the delivering consumer |
| `DELETE /{session_id}` | — | Close without acknowledging pending records |

The client workflow is **receive → process → validate → persist state and
outputs → acknowledge**. Position responses use last-consumed offsets. The
server never accepts raw offsets in a receipt request. It cannot prove that
an application persisted its checkpoint; that ordering remains the caller's
responsibility. A rebalance after validation can still reject acknowledgement
after persistence: recover using the persisted checkpoint and broker redelivery.

Repeated open requests with the same caller, subscription, and `request_id`
return the same session during its lifetime and closed-session retention.
A closed retained key returns `410`; after retention a fresh session may be
created with a new server UUID. Retrying receive returns cached pending receipt
IDs; additional records can appear as the consumer prefetches. Successful ack
retries are confirmed from bounded history in the same assignment. Unknown or
history-evicted IDs return `404`; tracked revoked IDs and processing gaps return
`409`. Expired or closed sessions reject work with `410`. Authentication and
subscribe grants are checked on every request. Receipt access is also bound to
the caller ID, authentication method, exact scope, and subscription definition.

Configure resource limits under `[bus.sessions]`:

```toml
[bus.sessions]
max_sessions = 512
max_sessions_per_tenant = 64
max_in_flight = 256
max_buffer_bytes = 8388608
max_ack_history = 1024
idle_timeout_ms = 60000
lifetime_ms = 300000
closed_retention_ms = 30000
```

Capacity includes closed entries during retention. Each session has a bounded
32-command queue; full registry or queue returns `429`. Receive limits cannot
exceed `max_in_flight`; `wait_ms` cannot exceed 30,000. Consumer opening has a
10-second deadline. Payload bytes cover serialized messages retained by the
server; librdkafka's queues and HTTP response buffers have separate limits.
Idle expiry, absolute lifetime, and the subscription's `ack_timeout_ms` close
the consumer and discard pending capabilities **without committing**. Invalid
broker JSON and excess payload bytes also close it. These paths do not perform
automatic dead-letter routing or automatic acknowledgement.

Consumers poll in the background while record and payload capacity remains.
A full batch pauses polling; process and acknowledge it before Kafka's maximum
poll interval or the configured receipt timeout. A full consumer does not make
an expired assignment safe to commit. Broker membership fencing remains active.

The registry is **process-local**. Use sticky routing for all requests belonging
to a session. Another replica or a restarted server returns `404` for an old
session; it never opens a substitute consumer to commit those receipts. Open a
new session and restore the durable application checkpoint. Subscription
removal cancels local sessions, and later requests reload the subscription
record. Closing or disconnecting during an already-started commit cannot undo
that commit; retry or reconcile broker progress. Sessions do not provide an
atomic Kafka/state-store transaction or exactly-once external effects.

The Rust client provides `open_bus_session`, `get_bus_session`,
`receive_bus_session`, `validate_bus_receipts`, `acknowledge_bus_receipts`, and
`close_bus_session`. Other clients can use the documented JSON endpoints.
