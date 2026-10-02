# Event-Time Windows

`acteon-bus` provides a reusable event-time window operator for joining records
from several streams before dispatching a decision. It works with both Kafka
and the in-memory bus because it consumes normalized records rather than a
broker-specific consumer.

Use it for telemetry correlation, fraud signals, order and payment joins,
multi-agent evidence collection, or any flow that needs bounded, replay-safe
state across sources.

## Data model

Each [`WindowRecord`](https://docs.rs/acteon-bus/latest/acteon_bus/struct.WindowRecord.html)
contains:

- a globally unique `event_id` for replay deduplication;
- a configured source name;
- a correlation key shared by related records;
- the application event timestamp;
- the original or normalized JSON payload; and
- its topic, partition, and offset.

`WindowRecord::from_bus_message` requires the broker position and preserves it
in the emitted window. The consumer can therefore include window state, ready
outputs, and source offsets in one recovery checkpoint before committing the
offsets.

## Configure the operator

```rust
use acteon_bus::{EventTimeWindowAggregator, EventTimeWindowConfig};
use chrono::Duration;

let mut config = EventTimeWindowConfig::new(
    ["metrics", "traces", "logs"],
    Duration::minutes(1),
    Duration::seconds(15),
)?;

config.emit_when_complete = true;
config.max_open_windows = 20_000;
config.max_records_per_window = 1_000;
config.max_records_per_source_per_window = 100;
config.max_tracked_event_ids = 200_000;

// After Kafka metadata discovery, track each partition independently.
config.set_source_partitions("metrics", [
    ("observability.acme.metrics", 0),
    ("observability.acme.metrics", 1),
])?;

let mut windows = EventTimeWindowAggregator::new(config)?;
# Ok::<(), acteon_bus::EventTimeWindowError>(())
```

Windows are fixed and aligned to the Unix epoch. Records join on correlation
key plus window start. A window can retain multiple records from each source.
Emission sorts those records by event time and broker position, independent of
their cross-partition arrival order. `idempotency_key()` returns a stable key
from the correlation key and window start for an outbox or downstream dispatch.

Early completion works best when each source publishes one aggregate per
window. Set `emit_when_complete = false` when every record through the watermark
must be included; otherwise, a later record for an already complete window is
correctly classified as late.

The capacities are hard admission limits. The per-source limit can enforce one
aggregate per source by setting it to `1`. Exceeding a limit returns a typed
error before the new record is admitted, allowing the consumer to apply
backpressure or route the record to a dead-letter topic.

## Watermarks and lateness

By default, ingesting a record advances one logical clock for its source. For a
partitioned source, call `set_source_partitions` with the complete assignment
before constructing the operator. Each declared topic partition then advances
independently, so a fast partition cannot hide lag on another partition. A
record from an undeclared partition is rejected.

The global watermark is:

```text
minimum(high-water time for every configured watermark lane) - allowed lateness
```

Once the watermark reaches a window's exclusive end, Acteon emits that window.
The result can be incomplete and exposes its missing sources explicitly. With
`emit_when_complete = true`, a window containing at least one record from every
source is emitted immediately.

A quiet source can report progress without manufacturing an event:

```rust
let ready = windows.advance_source_watermark("logs", observed_through)?;
# Ok::<(), acteon_bus::EventTimeWindowError>(())
```

For a partitioned source, pass a `WindowWatermarkLane::partition(...)` to
`advance_watermark` instead.

This makes idleness policy an operator decision. A source adapter can advance
the watermark from a broker partition watermark, a heartbeat, or a bounded
processing-time fallback. Validate producer timestamps before ingestion; one
far-future event can legitimately advance its source clock and affect late-data
decisions.

## Replay and recovery

The operator rejects duplicate event IDs and never reopens an emitted window.
Call `snapshot()` to serialize:

- open windows and all source positions;
- early-finalized windows still inside the lateness horizon;
- the event-ID deduplication ledger;
- per-source or per-partition high-water times;
- capacity and timing configuration; and
- cumulative counters.

`EventTimeWindowAggregator::restore` validates the complete snapshot before it
accepts another record. It rejects duplicate entries, invalid boundaries,
unknown sources, records outside their assigned window, inconsistent dedup
state, capacity violations, and windows that should already have emitted.

For crash-safe Kafka processing, use
[`StreamCheckpointCoordinator`](stream-checkpoints.md) to enforce this order:

1. Ingest source records and collect emitted windows.
2. Persist the operator snapshot and ready-window outbox atomically.
3. Commit each included Kafka offset.
4. Deliver ready windows with an idempotent output key.
5. Remove delivered outbox entries in the next checkpoint generation.

The window snapshot is a state-machine boundary. The checkpoint coordinator
stores it with offsets and ready outputs through any Acteon state backend and
uses compare-and-swap to reject stale writers.

## Observability

`EventTimeWindowStats` reports accepted, duplicate, and late records plus
complete and incomplete windows. Export these with consumer lag, open-window
count, watermark age, checkpoint duration, and capacity errors. Together they
show whether a source is stalled, clocks are skewed, or the configured bounds
are too small.

The [neural observability detector](../guides/neural-observability-detector.md)
uses this operator through a thin telemetry-schema adapter. Its Kafka recovery
simulation checkpoints the public snapshot before committing offsets and then
proves that replayed records are deduplicated after restart.
