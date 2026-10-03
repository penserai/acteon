# Neural observability detector

This runnable example consumes metrics, traces, and logs from three Kafka
topics, correlates them in 60-second event-time windows, evaluates each window
with four real [Laya](https://github.com/NandhaKishorM/laya) System One calls,
then sends a closed verdict through Acteon's rules. Three signal decisions run
concurrently; their typed outputs feed a fourth fusion decision.

The four trials cover:

| Trial | Expected policy |
|---|---|
| Healthy baseline | suppress |
| Log-only noise | suppress |
| Correlated database pool exhaustion | run the incident response chain |
| Missing logs with contradictory evidence | invoke the bounded investigator |

The detector does not trust the fusion choice by itself. It requires metrics,
traces, and logs to corroborate a known incident before starting a chain. This
matters in the measured run: Laya identified the first-stage signals well, but
its low-confidence fusion answer still chose a non-healthy incident for healthy
inputs. Acteon's deterministic policy prevented those raw false positives from
causing side effects.

## Prerequisites

- Docker with Compose v2
- Rust 1.88 or newer
- 8 GB of memory and 10 GB of free disk for Laya's CPU quickstart
- ports 8000, 16379, and 19092 available on loopback

The image explicitly installs PyTorch from its CPU wheel index. A plain
`pip install` on Linux can otherwise select CUDA dependencies even for a
CPU-only service.

## Run it

From the repository root:

```bash
examples/neural-observability-detector/scripts/run.sh
```

The script:

1. builds the pinned CPU Laya service and verifies runtime and model artifacts;
2. starts Kafka and Redis with AOF persistence, then publishes three telemetry streams;
3. uses `EventTimeWindowAggregator` and Redis-backed `StreamCheckpointCoordinator`
   to persist window state, ready outputs, and source positions;
4. replaces the consumer/coordinator before committing Kafka offsets, restores
   from a fresh Redis connection pool, and rejects five replayed broker positions;
5. invokes four locked question sets through `GovernedModelProvider` and the
   real gateway, validating a content-addressed response schema and served
   model revision on every call;
6. persists each completed verdict and its inference evidence before acknowledging
   the corresponding input window;
7. dispatches verdicts through `StreamOutboxDispatcher` and `dispatch_durable`,
   combining durable receipts and verdict routing in one gateway;
8. loses one acknowledgement after the incident chain completes, replaces the
   delivery worker and receiver gateway, and proves that receipt recovery repeats
   neither inference nor side effects;
9. retains a malformed verdict in dead-letter storage, inspects and discards it,
   and verifies zero pending outputs; and
10. writes measured JSON and Markdown reports.

Set `KEEP_LAYA=1` to leave the container running after the script exits. The
named model-cache volume persists between runs.

To run the pieces separately:

```bash
cd examples/neural-observability-detector
docker compose up -d --build --wait

cd ../..
cargo run -p acteon-simulation \
  --features bus,redis --example neural_observability_simulation -- --write-results
```

`LAYA_URL` defaults to `http://127.0.0.1:8000`, and `LAYA_API_KEY` defaults to
the loopback-only demo key in `docker-compose.yml`. `ACTEON_KAFKA_BOOTSTRAP`
defaults to `127.0.0.1:19092`. `ACTEON_CHECKPOINT_REDIS_URL` defaults to
`redis://127.0.0.1:16379`. Each run uses an isolated Redis key prefix.

## What is real

- Every detector and fusion answer comes from the local Laya checkpoint.
- The container refuses to serve when a runtime version, configured revision,
  checkpoint file set, or artifact SHA-256 differs from `model.lock.json`. The
  Rust runner verifies all four question sets and the response-schema digest.
  The governed providers recheck the health-reported revision before every call.
- The response validator consumes the unedited Laya JSON response.
- Acteon's real rules, gateway, chain executor, and Redis dispatch receipts
  process verdicts. The notification chain step uses full-pipeline dispatch;
  a reroute rule sends it to on-call, leaving its initial intake provider unused.
- Recording providers stand in for diagnostics, on-call, and the investigation
  agent, so the example produces no external operational side effects.
- Fixed JSON fixtures are published through the real Kafka backend. The
  event-time correlator rejects duplicate event IDs, retains broker positions,
  closes complete windows immediately, and closes incomplete windows after the
  15-second allowed-lateness watermark. The deliberate duplicate is published
  beside its original event before its event-ID retention watermark advances.
  Restart redeliveries are skipped using persisted source offsets before ingestion.
- Recovery and output delivery use platform checkpoint/outbox APIs backed by
  Redis. Window checkpoints precede Kafka commits. Delivery policy, attempts,
  retry deadlines, and dead letters survive replacement of the worker and its
  Redis connection pool. Four cached decisions survive, and final source lag,
  pending-window outputs, and pending-verdict outputs are zero.
- All three telemetry consumers use the public Acteon HTTP receipt-session API
  through the Rust client. The simulation registers topics and receipt-required
  subscriptions over HTTP, replaces the server after the pre-commit checkpoint,
  rejects its old session ID, and restores three consumers. It validates opaque
  receipt IDs to derive checkpoint positions, persists state and outputs, then
  acknowledges 13 source receipts through their delivering consumers.
- An independent library group probe joins a second member, observes rebalance
  revocation, rejects a stale receipt, and receives the uncommitted prefix from
  offset zero. It adds no model calls or effects. The server also rejects raw
  offset acknowledgements for receipt-required subscriptions.

The acknowledgement-loss fault happens **after completed dispatch**. The retry
recovers the original durable dispatch receipt and chain ID through a replacement
receiver gateway with a fresh Redis pool. Recording providers survive as external
observers. Gateway tests additionally cover interruption before chain creation
and after chain completion but before receipt completion, concurrent retries,
changed payload/caller conflicts, and expired-attempt fencing.

Interrupted external provider calls require explicit reconciliation. Admission
cannot undo their effects. Receipts and admitted chain state have no automatic
TTL; plan storage maintenance for deployed receivers.

Model timings are now governed model HTTP elapsed times, including response
validation. Full wall times also include runtime identity checks and gateway
handling. Automatic model and verdict-provider retries are disabled; model
failures stop the run, while the managed outbox owns delivery retries. They are
not comparable to the previous server-only inference timings.

The results are an integration demonstration. The shipped checkpoint reports
uncalibrated confidence for part of this question shape, and these four fixtures
are not evidence of production precision or recall.

## Layout

```text
docker-compose.yml       Kafka, Redis AOF storage, and CPU Laya service
laya/Dockerfile          CPU-only PyTorch and pinned Laya package
laya/verify_and_serve.py Fail-closed runtime and artifact verifier
fixtures/                Four fixed telemetry windows
questions/               Metrics, traces, logs, and fusion question sets
contracts/               Content-addressed Laya response schema
rules/                   Deterministic Acteon routing
scripts/run.sh            One-command simulation
model.lock.json           Runtime, model artifact, and question-set identity
results/                  Measured JSON and Markdown reports
```

The reusable event-time state machine and its replay, lateness, missing-source,
idle-source, snapshot, and capacity tests live in
`crates/bus/src/windowing.rs`. The simulation-specific telemetry adapter lives
beside the Rust example in
`crates/simulation/examples/neural_observability/windowing.rs`.
The checkpoint module beside the example now only converts telemetry envelopes
and broker positions. Storage, CAS, leases, retries, and dead letters come from
`acteon-bus::StreamCheckpointCoordinator` and `StreamOutboxDispatcher`.

All 16 model requests go through four `acteon-llm::GovernedModelProvider`
instances registered with a gateway. Each provider injects its locked questions
and model name, verifies runtime identity, and uses `TypedJsonModelClient` for
response-schema validation. The report preserves parsed Laya responses, model
request and response contract names, lock digests, revisions, and elapsed times.
