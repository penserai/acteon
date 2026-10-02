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
- ports 8000 and 19092 available on loopback

The image explicitly installs PyTorch from its CPU wheel index. A plain
`pip install` on Linux can otherwise select CUDA dependencies even for a
CPU-only service.

## Run it

From the repository root:

```bash
examples/neural-observability-detector/scripts/run.sh
```

The script:

1. builds and starts the local Laya 0.3.23 service;
2. starts Kafka and publishes the fixed telemetry envelopes to separate
   metrics, traces, and logs topics;
3. consumes and correlates the records by event time and writes an atomic
   checkpoint containing active state, ready outputs, and source offsets;
4. injects a crash before the Kafka commit, restores the checkpoint, rejects
   the uncommitted redeliveries, then checkpoints before committing offsets;
5. waits until the reviewed `typed-decisions` checkpoint is resident and runs
   the Rust simulation against `POST /v1/systemone`;
6. validates question IDs, answer types, labels, probability ranges and sums,
   routing identity, and zero output tokens;
7. executes Acteon suppression, reroute, and chain paths with recording
   providers;
8. replays the incident key and verifies that the runner ledger prevents a
   second dispatch; and
9. writes `results/latest.json` and `results/latest.md`.

Set `KEEP_LAYA=1` to leave the container running after the script exits. The
named model-cache volume persists between runs.

To run the pieces separately:

```bash
cd examples/neural-observability-detector
docker compose up -d --build --wait

cd ../..
cargo run -p acteon-simulation \
  --features bus --example neural_observability_simulation -- --write-results
```

`LAYA_URL` defaults to `http://127.0.0.1:8000`, and `LAYA_API_KEY` defaults to
the loopback-only demo key in `docker-compose.yml`. `ACTEON_KAFKA_BOOTSTRAP`
defaults to `127.0.0.1:19092`.

## What is real

- Every detector and fusion answer comes from the local Laya checkpoint.
- The response validator consumes the unedited Laya JSON response.
- Acteon's real rule engine, gateway, chain executor, memory state, and locks
  process the admitted verdict.
- Recording providers stand in for diagnostics, on-call, and the investigation
  agent, so the example produces no external operational side effects.
- Fixed JSON fixtures are published through the real Kafka backend. The
  event-time correlator rejects duplicate event IDs, retains broker positions,
  closes complete windows immediately, and closes incomplete windows after the
  15-second allowed-lateness watermark.
- Recovery uses a versioned atomic file checkpoint. The simulation restores
  after a crash before offset commit, keeps ready windows in the checkpoint as
  an outbox, deduplicates the replayed prefix, and verifies zero Kafka lag only
  after the final checkpoint-before-commit sequence.

The results are an integration demonstration. The shipped checkpoint reports
uncalibrated confidence for part of this question shape, and these four fixtures
are not evidence of production precision or recall.

## Layout

```text
docker-compose.yml       Local Kafka broker and CPU Laya service
laya/Dockerfile          CPU-only PyTorch and pinned Laya package
fixtures/                Four fixed telemetry windows
questions/               Metrics, traces, logs, and fusion question sets
rules/                   Deterministic Acteon routing
scripts/run.sh            One-command simulation
model.lock.json           Model, runtime, and question-set identity
results/                  Measured JSON and Markdown reports
```

The event-time state machine and its replay, lateness, and missing-source tests
live beside the Rust example in `crates/simulation/examples/neural_observability/windowing.rs`.
The atomic store, recovery envelope, and checkpoint-before-commit tests live in
`crates/simulation/examples/neural_observability/checkpoint.rs`.
