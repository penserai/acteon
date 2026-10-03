# Consume Contracts and Poison-Record Quarantine

Managed stream stages can validate consumed payloads against pinned JSON Schema
contracts before invoking their processor. Publish-edge validation protects
Acteon's HTTP producers; consume validation also covers records from other Kafka
producers and existing broker history.

`StreamStageConfig.input` contains the contracts and failure policy. Each contract
pins namespace, tenant, subject, version, schema body, and a canonical SHA-256
digest. The policy is persisted with the checkpoint. Restarting with a different
schema body, version, or failure policy returns `DefinitionMismatch`. Validators
compile during initialization; external references cannot fetch files or network
resources. Self-contained `$defs` and local references are supported, and known
JSON Schema formats are asserted.

## Configure a managed stage

For native input, build `StreamInputContract::from_schema(&acteon_core::Schema)`.
For HTTP input, fetch an explicit registry version and use the subscription's
`input_contract(&BusSchema)` helper; it rejects schemas from another tenant or
namespace. Bind contracts by logical source name:

```rust,ignore
use acteon_bus::{StreamInputPolicy, StreamPoisonPolicy, StreamStageConfig};
use std::collections::BTreeMap;

let schema = client.get_bus_schema("observability", "acme", "metrics", "1").await?;
let contract = metrics_subscription.input_contract(&schema)?;
let config = StreamStageConfig {
    input: StreamInputPolicy {
        contracts: BTreeMap::from([("metrics".into(), contract)]),
        poison_policy: StreamPoisonPolicy::Quarantine,
        max_quarantined_records: 100,
        max_quarantine_bytes: 1024 * 1024,
    },
    ..Default::default()
};
```

If the contract map is nonempty, every fresh input source must have a contract.
An unbound source is a configuration error, not a poison record. An empty map
preserves the existing typed-decode behavior. A contract's version is explicit;
a running stage does not track the registry's latest version or a mutable topic
binding.

## Failure policies

`Halt` is the default. Schema violations and typed-decode failures halt without
acknowledging input. `Quarantine` retains those individual rejected records and
passes the remaining valid inputs to the processor. An all-rejected batch invokes
no processor. Processor failures, model outages, exhausted callback retries, and
oversized outputs retain their existing halt/retry behavior; they do not silently
quarantine an entire batch of otherwise valid telemetry.

Every quarantine entry preserves the consumed envelope, source/topic/group/
partition/offset, failure class, failure time, contract digest, and a bounded
diagnostic. Diagnostics omit payload values; the retained envelope contains the
original payload and must receive the same storage/access treatment as telemetry.
Receipt capabilities are never stored in quarantine entries.

The stage validates the complete receipt prefix and saves **state, outputs,
quarantine entries, and input positions together in one CAS checkpoint** before
acknowledging any input. Invalid data can therefore advance past the consumer
only after its retained copy is durable. If checkpoint persistence fails, input
is not acknowledged. A lost checkpoint reply or lost source acknowledgement
recovers through stored positions: replay creates no second quarantine entry and
invokes no processor for saved inputs. This uses the same receipt fencing as
ordinary managed processing.

## Capacity and inspection

Both retained-entry count and serialized-byte limits are persisted policy. Limits
cover complete envelopes and diagnostics. When a rejected batch would exceed
either limit, the stage reports `Backpressured` before running its processor or
spending a new callback attempt. It keeps input unacknowledged and never evicts an
older entry automatically. Choose byte limits that fit the selected state store's
row limits as well as its operational storage budget.

```rust,ignore
let entries = stage.quarantined_inputs().await?;
// After inspecting an entry and deciding it can be removed:
let removed = stage.discard_quarantined_input(&entries[0].id).await?;
```

Inspection reloads durable state. Discard is explicit, CAS-protected, idempotent,
and increments a durable discard counter. It does not rewind Kafka, dispatch an
action, or invoke the processor. Concurrent input processing preserves retention
changes when it saves its transition. Operator HTTP endpoints and controlled
replay are separate follow-ups; quarantine inspection/discard are currently Rust
library APIs.

`StreamStageMetrics` includes retained quarantine count and cumulative retained /
discarded counters. `processed_records` counts fresh broker inputs whose progress
was checkpointed, including quarantined records; it is not a model invocation
count. Model or processor calls need their own invocation evidence.

Checkpoints with consume policy use snapshot version 4. Existing versions 1–3
remain readable, and stages using the default input policy retain version 3.
Changing a live stage's policy requires a deliberate checkpoint migration or a
new processing key and consumer group.

The [cascading observability simulation](../guides/neural-observability-detector.md)
registers three schemas, injects a malformed Kafka log, and demonstrates durable
quarantine across server replacement while valid telemetry reaches real Laya
inference.
