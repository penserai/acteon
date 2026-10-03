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
changes when it saves its transition. Inputs with pending/running replay requests
cannot be discarded (409), so a worker never loses the original during repair.

### Operator HTTP APIs

These endpoints read the durable checkpoint even when Kafka is unavailable. They
never open consumers, acquire processing leases, or create missing stages.

| Method | Path suffix under `/v1/bus/stages/{namespace}/{tenant}/{id}` | Result |
|---|---|---|
| GET | (none) | Counters, checkpoint generation, pending outputs, retained count, lease expiry, retry/halt status |
| GET | `/quarantine?limit=50&after=UUID` | Metadata only; at most 100 entries, with `next_after` cursor |
| GET | `/quarantine/{entry}` | One complete original envelope and failure metadata |
| DELETE | `/quarantine/{entry}` | `{ "discarded": true }`, or false if already absent |

Reads require a grant for the target namespace/tenant with `provider=bus` and
`action=stage_read`. All roles can read with that grant. Discard requires an
admin/operator role and a separate `action=stage_manage` grant. Existing subscribe
and topic management grants do not authorize stage operations. Original envelopes
may contain sensitive telemetry; grant read access accordingly.

Status omits processor state, output payloads, source identity, and lease tokens.
A missing stage or input returns 404. An unmanaged checkpoint returns 409.
Namespace and tenant components reject `:` and control characters to prevent
storage-key aliases. Processor IDs must be nonempty and at most 4096 bytes. Pages
reflect current retained state rather than a frozen snapshot; if a cursor was
discarded between requests, the API returns 409 and the caller restarts listing.
Requests have a five-second storage deadline. A timed-out discard may have
committed: inspect or repeat the idempotent request.

The Rust SDK exposes `stream_stage_status`, `list_stage_quarantine`,
`get_stage_quarantined_input`, and `discard_stage_quarantined_input` with the
`stream-processing` feature. `StreamStageOperator::load` provides the same durable
access for embedded operators without knowing processor state/output types.

`StreamStageMetrics` includes retained quarantine count and cumulative retained /
discarded/replayed counters, plus pending and failed replay counts. `processed_records` counts fresh broker inputs whose progress
was checkpointed, including quarantined records; it is not a model invocation
count. Model or processor calls need their own invocation evidence.

Checkpoints with consume policy use snapshot version 4; durable replay requests
and custom replay retention limits require version 5. Existing versions 1–4 remain readable, and stages using the
default input policy retain version 3. Deploy replay-capable workers before
enabling replay requests: older workers reject version 5.
Changing a live stage's policy requires a deliberate checkpoint migration or a
new processing key and consumer group.

The [cascading observability simulation](../guides/neural-observability-detector.md)
registers three schemas, injects a malformed Kafka log, and demonstrates durable
quarantine across server replacement while valid telemetry reaches real Laya
inference.


### Controlled, audited replay

Replay is an explicit repair processed by the existing leased stage worker. It
never rewinds Kafka or acknowledges old receipt capabilities. The original
quarantine stays retained until the repaired input, processor state, outputs,
and completed audit are saved in one CAS checkpoint. Source positions stay
unchanged. Outputs flow through the same durable outbox as normal processing.

`POST /v1/bus/stages/{namespace}/{tenant}/{id}/quarantine/{entry}/replay`
requires an admin/operator role and a distinct `provider=bus`,
`action=stage_replay` grant. Topic management and discard grants do not authorize
re-execution. The body contains a non-nil `request_id` UUID, a nonempty `reason`
(up to 4096 bytes), and a corrected JSON `payload`. The authenticated server
identity supplies the actor; callers cannot impersonate another operator in the
body. The original envelope metadata and position remain fixed.

Admission validates the pinned consume schema and input byte bound. The worker
also validates that schema and decodes the actual processor input type before
invoking the callback. Invalid admission returns 400 without creating a request;
a type/processor failure produces a durable failed audit and retains the original.

The POST returns 202 with a durable audit. Repeat the same UUID, entry, actor,
reason, and payload after a timeout; the API returns the original request, even
after completion. Changing any of those values under the same UUID returns 409.
Only one active request per input is allowed. A failed request can be followed
by a new explicit request ID while its original remains retained.

Read `GET /v1/bus/stages/{namespace}/{tenant}/{id}/replays/{request_id}` with a
`stage_read` grant. Its payload-free audit records the operator, reason, original
position, processor version, pinned contract digest, original/repaired payload
digests, attempts, retry time, error, outcome time, and completed generation.
States are `pending`, `running`, `completed`, and `failed`.

The normal `process_once`/`run` loop drains ready repairs before receiving a
source batch. Embedded workers can call `replay_once` without connecting to
Kafka. Replays respect the stage lease, halt state, output headroom, processing
timeout, and persisted attempt/backoff policy. Interruptions before checkpoint
may repeat the callback; use pure/idempotent callbacks. A persisted completed
request cannot execute again, and an expired worker cannot overwrite its
replacement's outcome. Retry attempts survive worker replacement; exhausting
them fails the request and preserves the original. Pending backoff does not
block normal source input.

`StreamStageConfig.max_replay_requests` (default 1000) and
`max_replay_bytes` (default 16 MiB) bound retained requests **including terminal
audit history and reserved space for error outcomes**. History is never silently
evicted to accept a new request; capacity exhaustion returns 429. Choose bounds
that fit the state store together with processor state, quarantine, and outbox.
Archival/pruning of this audit history remains a separate lifecycle feature.

The Rust SDK exposes `request_stage_replay` and `get_stage_replay_audit` with
`stream-processing`. Embedded operators use `StreamStageOperator::request_replay`
and `replay_audits`.

Repair does not override domain rules. In the observability simulation, a repair
of the poison log is queued twice under one ID and completed by a replacement
worker. The window operator deduplicates its event ID while the window is still open. The audit completes once, quarantine becomes
empty, and no extra Laya inference or incident occurs.
