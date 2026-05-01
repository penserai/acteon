# Agentic Bus — Benchmarks

> Phase 9 micro-benchmarks measuring the cost of typed bus
> envelopes vs a raw `BusMessage` publish on the in-memory
> backend. The interesting number is the *delta*: what does the
> typed-envelope layer cost the producer process before the
> message hits the wire?

## Why these benchmarks

A real Kafka deployment adds network + broker latency on the
order of milliseconds. Anything Acteon does in-process — payload
serialization, header stamping, validation — sits in the
microsecond range and is in the noise of the broker round-trip.

That makes "Acteon vs Kafka direct" a hard number to make
meaningful — it's dominated by what neither party controls.
Instead, the bench measures **what Acteon controls**: the cost
of going from a typed envelope (`ToolCall`, `StreamChunk`,
`StreamEnd`) to a wire-ready `BusMessage`. Subtract the raw
publish baseline and you have the typed-layer overhead exactly.

## Methodology

- Backend: `MemoryBackend` (no Kafka, no network). Reveals
  Acteon-side overhead independent of broker latency.
- Tokio runtime: single-threaded, fresh `Runtime::new` per
  bench group.
- Payload: small JSON (~1–2 fields, ≤ 100 bytes).
- Headers: the same `acteon.*` headers the production handlers
  stamp. No additional user labels.
- Each bench includes the full path the production handler runs:
  construct the typed envelope, validate, serialize to
  `serde_json::Value`, build the `BusMessage`, stamp routing
  headers, call `backend.produce`.

Run locally:

```text
cargo bench -p acteon-simulation --features bus --bench bus_overhead
```

## Reference numbers

Single-iteration timings on an Apple M-series laptop, release
build, in-memory backend. Consider these the **shape** of the
overhead, not absolute performance promises — they'll vary by
~1.5–2× across different hardware.

| Bench | Median time | Throughput |
|---|---:|---:|
| `bus/raw_publish` | ~360 ns | ~2.78 M ops/s |
| `bus/post_tool_call` | ~1.68 µs | ~596 K ops/s |
| `bus/post_tool_result` | ~1.73 µs | ~577 K ops/s |
| `bus/post_stream_chunk` | ~1.28 µs | ~782 K ops/s |
| `bus/post_stream_end` | ~1.06 µs | ~945 K ops/s |
| `bus/validate/tool_call` | ~15 ns | — |

Read this way:

- The typed-envelope layer adds **~700 ns to ~1.3 µs** of
  in-process overhead per envelope vs a raw publish — payload
  serialization + header stamping + validation, dominated by the
  JSON round-trip.
- `validate()` itself is ~15 ns and is in the noise; the bulk of
  the typed-layer cost is `serde_json::to_value` on the envelope.
- Stream chunks are slightly cheaper than tool envelopes because
  their payloads are smaller (a single token rather than a tool
  result body).

## Implications for production capacity

For a busy LLM streaming workload at, say, 1000 tokens/second
across 100 concurrent streams (100K chunks/second total): the
typed-layer overhead is ~130 ms of CPU per second, ~13% of one
core. The Kafka produce itself dominates wall-clock — Acteon's
overhead is well below it.

For a tool-call-heavy planner doing ~100 calls/second: ~170 µs
of CPU per second, well under a percent of a core.

The takeaway: **the typed envelope layer is not a bottleneck on
realistic agent workloads.** The first bottleneck you'll hit
is the broker round-trip; the next is your model's token-emit
rate.

## When to re-run the bench

The benchmark harness lives at
`crates/simulation/benches/bus_overhead.rs`. Re-run after any
change that touches:

- `crates/core/src/bus_*.rs` (envelope types — adding a field
  changes the JSON serialization cost).
- `crates/server/src/api/bus.rs` (the production handlers — though
  the bench mirrors them rather than calling them, drift between
  the two should be caught here).
- `crates/bus/src/memory.rs` (the in-memory backend; if its
  produce path gets faster or slower, all numbers shift).

Criterion's `--save-baseline` lets you compare runs:

```text
cargo bench -p acteon-simulation --features bus --bench bus_overhead -- --save-baseline before
# ... make changes ...
cargo bench -p acteon-simulation --features bus --bench bus_overhead -- --baseline before
```

A regression of more than ~10% is worth investigating. The bench
is fast enough (~6 minutes for a full run) that running it on
every PR that touches bus code is reasonable.

## What this bench doesn't measure

- **Broker latency.** This is in-memory only. A real Kafka
  cluster adds milliseconds of network + replication.
- **Subscriber-side cost.** The bench measures producer overhead.
  Header-filtering on a busy events topic also costs cycles
  (per-record hashmap lookups); a separate consumer-side bench
  would be the natural follow-up.
- **End-to-end latency.** A tool-call → result round-trip
  involves two produces, a topic scan, and a deserialization.
  Modeling that requires a Kafka instance and is out of scope
  for this in-process bench.

A `bus_kafka_e2e` bench against `KafkaBackend` is on the
roadmap; it'd run under the existing Docker `kafka` profile and
report wall-clock numbers including broker latency.
