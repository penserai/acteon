# Cascaded Neural Observability Detector

This guide designs an observability pipeline that uses
[Laya](https://github.com/NandhaKishorM/laya), an open-source,
non-autoregressive decision model, for the common case and invokes a generative
agent only when evidence is both important and ambiguous. Metrics, traces, and
logs arrive on separate Kafka topics. Signal-specific typed question sets score
them independently, a fusion question set correlates the evidence, and Acteon
applies deterministic policy before it starts an incident chain or an
investigation agent.

The result is a **System One-style detector**: a fast, non-autoregressive path
that handles repetitive pattern recognition without paying the latency, token
cost, or hallucination risk of an LLM on every event.

!!! note "What System One means here"
    This guide uses *System One* as an architectural shorthand for bounded,
    feed-forward inference with a fixed output schema. It is not a claim that
    a classifier reproduces human cognition.

!!! important "Integration boundary"
    This example places a small **detector runner** between Kafka and Acteon.
    It owns telemetry correlation, feature extraction, Laya calls, response
    validation, and the final `detector.verdict` dispatch. Acteon owns policy,
    audit, chains, approvals, and agent invocation.

---

## The scenario

A deployment of `checkout-api` exhausts its database connection pool. No single
signal is conclusive:

| Source | Observation in one 60-second window | Detector result |
|---|---|---|
| Metrics | p95 latency rises from 180 ms to 1.8 s, errors reach 8.4%, DB pool utilization reaches 96% | `resource_saturation`, score `0.94` |
| Traces | 73% of slow requests spend most of their time in `db.checkout.reserve` | `downstream_dependency`, score `0.88` |
| Logs | 41 `pool_timeout` events appear, mixed with unrelated warnings | `db_pool_exhaustion`, score `0.91` |

The fusion evaluation returns a typed verdict:

```json
{
  "schema_version": 1,
  "incident_key": "checkout-api:prod:2026-10-01T19:42Z",
  "service": "checkout-api",
  "environment": "prod",
  "kind": "db_pool_exhaustion",
  "score": 0.96,
  "uncertainty": 0.07,
  "impact": "high",
  "evidence": [
    {"source": "metrics", "ref": "metrics:4:8821", "score": 0.94},
    {"source": "traces", "ref": "traces:2:1904", "score": 0.88},
    {"source": "logs", "ref": "logs:7:4420", "score": 0.91}
  ],
  "model": {
    "repository": "convaiinnovations/laya",
    "revision": "<pinned-commit>",
    "checkpoint": "typed-decisions",
    "questions": "fusion@1"
  }
}
```

Acteon can now make a deterministic decision. This high-confidence verdict
starts an incident chain. A verdict with high impact and high uncertainty
invokes a bounded investigation agent. Low scores are recorded and suppressed.

---

## Architecture

```mermaid
flowchart LR
    M[(metrics topic)]
    T[(traces topic)]
    L[(logs topic)]

    subgraph Runner[Detector runner]
        N[Normalize and window]
        V[Validate typed verdict]
    end

    subgraph Models[Self-hosted Laya service]
        MD[Metric detector]
        TD[Trace detector]
        LD[Log detector]
        F[Fusion evaluation]
    end

    subgraph Acteon
        R[Rules]
        C[Incident action chain]
        A[Bounded investigation agent]
        H[Approval gate]
        AU[(Audit)]
    end

    M & T & L --> N
    N -->|typed HTTP calls| MD & TD & LD
    MD & TD & LD --> F
    F -->|typed HTTP response| V
    V -->|detector.verdict| R
    R -->|high score, low uncertainty| C
    R -->|high impact, high uncertainty| A
    R -->|risky remediation| H
    R -->|low score| AU
    C & A & H --> AU
```

The boundary between detection and orchestration is deliberate:

- Kafka and the detector runner own high-volume data movement, event-time
  semantics, late arrivals, windows, and model inference.
- Acteon owns policy, deduplication, throttling, audit, response chains,
  approvals, and agent governance.
- Agents receive a compact evidence bundle and references to source data. They
  do not receive an unbounded stream of raw telemetry.

---

## Comprehensive implementation plan

### 1. Define the decision and the error budget

Start with one incident class and one operational decision. For this scenario:

- **Incident class:** database connection-pool exhaustion.
- **Entity:** `(tenant, environment, service, deployment_revision)`.
- **Decision interval:** one verdict per 60-second event-time window.
- **Response:** open or update an incident, capture diagnostics, and notify the
  owning team.
- **Agent escalation:** investigate only when impact is high and uncertainty or
  detector disagreement exceeds policy.

Choose acceptance targets before selecting models:

| Measure | Initial simulation target |
|---|---:|
| Correlated incident recall | at least 95% |
| False incidents during baseline windows | at most 1% |
| Detection p95 after window close | under 2 seconds |
| Windows sent to an agent | under 2% |
| Invalid model responses admitted | 0 |
| Duplicate incident chains per incident key | 0 |

These are example targets, not production claims. Replace them with values
derived from incident cost, on-call capacity, and replay data.

### 2. Establish typed Kafka contracts

Use three source topics and one verdict topic:

| Logical topic | Key | Value |
|---|---|---|
| `observability.acme.metrics` | service + window | normalized metric sample or aggregate |
| `observability.acme.traces` | service + window | trace summary, not full span payloads |
| `observability.acme.logs` | service + window | log template counts and selected attributes |
| `observability.acme.verdicts` | incident key | validated fused verdict |

If producers can publish through the Acteon Agentic Bus, register a JSON Schema
for each payload and bind it to the topic. Publish-edge validation rejects
malformed records before they enter the detector. If telemetry already lives on
independently managed Kafka topics, deploy a bridge that normalizes those
records into the same contracts.

Every input contract should include:

- `schema_version`, `event_id`, `observed_at`, and `ingested_at`;
- tenant, environment, service, region, and deployment revision;
- a stable correlation key and a source reference containing topic, partition,
  and offset;
- a bounded feature payload with explicit units;
- trace context when it is available.

Do not send raw high-cardinality telemetry through Acteon's dispatch API. Keep
the large records in the observability store or Kafka retention window and pass
references plus bounded features to Acteon.

### 3. Build event-time correlation

The detector runner consumes all three topics with durable consumer groups. It
normalizes records into 60-second tumbling windows with a small allowed-lateness
period, such as 15 seconds. It joins by tenant, environment, service, region,
and deployment revision.

The runner must define these cases explicitly:

- missing signal: emit a mask rather than a fabricated value;
- late signal: update the window only within the allowed-lateness period;
- duplicate record: deduplicate by `event_id` or source coordinates;
- replay: make window output deterministic for the same ordered input;
- skewed clocks: prefer producer event time, while tracking ingest delay;
- overloaded service: apply backpressure and expose consumer lag.

A stream processor such as Kafka Streams or Flink is appropriate at large
volume. The vertical slice uses a small Rust service with a versioned atomic
checkpoint for the window map, ready-output outbox, deduplication ledger,
watermarks, and Kafka offsets.

### 4. Implement the fast detector cascade

Use Laya as the open-source System One evaluator. Laya accepts a state plus
typed `choice`, `score`, and `noul` questions and returns closed values and
probabilities in a single non-autoregressive forward pass. Its code and
published weights use Apache-2.0, and its server exposes the Jev-compatible
`POST /v1/systemone` protocol.

This is what happens in one Laya call:

1. The runner serializes one bounded state, such as the metric features for a
   60-second window.
2. It supplies several typed questions. `choice` selects one declared label,
   `score` returns an expected position on an ordered scale, and `noul` returns
   the probability that a yes/no proposition is true.
3. Laya encodes the state and rendered options and evaluates them in one model
   call. It emits probability distributions through decision heads. It does
   not decode an explanation token by token; API usage therefore reports zero
   output tokens.
4. The runner accepts only declared question IDs, labels, and numeric ranges.
   The model can still classify an incident incorrectly, so its probabilities
   require workload-specific evaluation and calibration.

The *cascade* is across calls. The first stage applies the same bounded decision
mechanism independently to metrics, traces, and logs. The second stage sends
only those three typed results, their provenance, and a missing-signal mask to
a fourth Laya call. Acteon then applies rules to the fused result. An
autoregressive agent enters the path only for a high-impact result whose
uncertainty or cross-signal disagreement requires investigation.

Use four versioned question sets over one pinned Laya checkpoint:

1. **Metric question set:** evaluates typed questions over rates, deltas, seasonal
   residuals, and saturation ratios.
2. **Trace question set:** evaluates a bounded service-graph and span-latency
   summary.
3. **Log question set:** evaluates template IDs and counts. Template mining
   happens before inference, so raw free-form logs do not enter Laya.
4. **Fusion question set:** evaluates the three typed signal results,
   missing-signal mask, deployment context, and recent incident state.

The question sets are versioned JSON, not instructions assembled by callers.
Each declares its accepted state schema, typed questions, criteria, and output
mapping. The runner converts Laya's response into the closed `SignalVerdict` or
fused-verdict schema.

Each signal detector returns:

```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalVerdict {
    schema_version: u16,
    source: SignalSource,
    label: SignalLabel,
    score: f32,
    uncertainty: f32,
    evidence_refs: Vec<String>,
    model: ModelIdentity,
}
```

Use smart constructors or explicit validation to enforce `score` and
`uncertainty` in `0.0..=1.0`, maximum evidence counts, allowed enum values, and
model-version allowlists. Serde types alone do not enforce numeric ranges.

The runtime is the upstream `laya-serve` HTTP service in a CPU container. The
example enforces its lock twice. The container verifies six installed package
versions, the configured repository and revision, the exact five-file
checkpoint manifest, and every artifact SHA-256 before it starts the server.
The Rust runner uses Acteon's reusable `acteon_llm::VerifiedModelLock` API to
parse the strict lock schema, load content-addressed inference contracts, and
verify the health-reported model and revision before its first inference call.
The same API can govern another inference engine, model registry, artifact
layout, or set of named contracts; the Laya startup script is the runtime
adapter for this deployment. Calls use Acteon's `TypedJsonModelClient`, which
checks the raw response against a compiled JSON Schema before deserializing it
to the declared Rust output type. The detector then applies its Laya-specific
semantic checks for question IDs, probabilities, and labels. Load only the
`typed-decisions` checkpoint to bound resident memory. Laya also supports ONNX
and per-channel INT8 export for a later compact CPU deployment, but the first
simulation uses the upstream server path. A future native Acteon provider can
remove the HTTP hop without changing the contracts.

### 5. Calibrate uncertainty and disagreement

Raw softmax scores are not uncertainty estimates. Calibrate each model on held
out incident and baseline windows, then monitor calibration drift. The fusion
input should include:

- calibrated score from each available detector;
- entropy or another bounded uncertainty measure;
- disagreement between top labels;
- number and freshness of available signals;
- deployment recency and known maintenance state;
- recent verdicts for the same incident key.

The runner should reject any verdict that is non-finite, outside its declared
range, from an unapproved model version, or missing evidence provenance.

### 6. Dispatch only the compact verdict to Acteon

The detector runner posts an Action such as:

```json
{
  "namespace": "observability",
  "tenant": "acme",
  "provider": "verdict-audit",
  "action_type": "detector.verdict",
  "payload": {
    "incident_key": "checkout-api:prod:2026-10-01T19:42Z",
    "service": "checkout-api",
    "environment": "prod",
    "kind": "db_pool_exhaustion",
    "score": 0.96,
    "uncertainty": 0.07,
    "impact": "high",
    "evidence": ["metrics:4:8821", "traces:2:1904", "logs:7:4420"],
    "model_version": "convaiinnovations/laya@<pinned-commit>:typed-decisions:fusion@1"
  },
  "dedup_key": "checkout-api:prod:2026-10-01T19:42Z",
  "fingerprint": "checkout-api:prod:db_pool_exhaustion"
}
```

The verdict schema is the security boundary. Free-form explanations may be
attached later as advisory evidence, but rules must depend only on typed fields.

### 7. Apply deterministic routing rules

Use rule priorities to separate the policy bands:

| Policy band | Example condition | Outcome |
|---|---|---|
| Invalid or unapproved | schema/model version not allowed | suppress and audit |
| Noise | `score < 0.55` | suppress and retain verdict |
| Watch | `0.55 <= score < 0.80` | notify or group |
| Incident | `score >= 0.80` and `uncertainty <= 0.20` | start `observability-incident` chain |
| Investigate | high impact and `uncertainty > 0.20` | invoke bounded agent |
| Remediate | chain or agent proposes a risky mutation | require approval |

Acteon's rules stop at the first matching rule, so order specific safety and
escalation rules before broad allow or suppress rules. One dispatch cannot both
match a deduplication rule and start a chain. For the vertical slice, the runner
must persist an idempotency record keyed by `(incident_key, policy_version)`
before it dispatches the verdict. A later implementation can split admission
and routing into two Actions through a chain-to-dispatch handoff. The
`dedup_key` remains useful metadata, but it does not by itself deduplicate an
Action handled by a `chain` rule.

### 8. Use an action chain for the known response

The high-confidence path should remain procedural:

1. create or update the incident;
2. capture a bounded diagnostics snapshot;
3. notify the service owner and on-call channel in parallel;
4. optionally perform a pre-approved reversible action;
5. wait for recovery evidence and resolve or escalate.

Chain templates can carry typed fields from earlier step results with
`{{prev.body.*}}` and `{{steps.NAME.body.*}}`. Pin chain and model versions in
the audit record. Put retry and circuit-breaker policy around external systems,
not around a model response that failed schema validation.

### 9. Invoke an agent only for ambiguity

The investigation agent receives a constrained goal:

```text
Determine which of these allowed hypotheses best explains the checkout-api
regression: db pool exhaustion, downstream timeout, deploy regression, or
insufficient evidence. Use only the attached evidence references and the
read-only observability tools. Return the InvestigationFinding schema. Do not
change production state.
```

Give it:

- the typed verdict and per-signal detector results;
- a small, time-bounded evidence bundle;
- read-only tools with tenant and service scope;
- a maximum runtime and token budget;
- a closed `InvestigationFinding` schema with hypothesis, confidence, evidence
  references, contradictions, and proposed next action.

The Ambient Swarm provider is suitable for asynchronous investigation and
returns a `run_id` immediately. Any proposed production mutation must come back
through Acteon as a new action, where rules, quotas, and approval gates apply.
Agent prose is never itself an authorization.

### 10. Measure the whole decision system

Record these fields for every window: source offsets, feature version, model
artifact digests, detector results, fused verdict, rule outcome, chain ID or
agent run ID, operator action, and eventual incident label.

Monitor:

- precision, recall, and time-to-detect by incident kind;
- calibration error and disagreement rate;
- missing-signal and late-arrival rates;
- Kafka lag and window completion latency;
- verdict-schema rejection rate;
- percentage of windows reaching each policy band;
- agent invocation rate, cost, runtime, and unsupported claims;
- chain success, retries, deduplication, and approval outcomes.

Deploy in shadow mode first. Compare verdicts with real incidents without
opening tickets. Then enable notifications, followed by incident creation, and
only later consider reversible remediation.

---

## Simulation: a convincing vertical slice

The smallest useful demonstration should prove transport, event-time
correlation, real neural inference, routing, agent containment, and
repeatability. It publishes synthetic telemetry through three Kafka topics and
invokes a pinned Laya checkpoint locally. The broker traffic and all four Laya
forward passes are real. Results demonstrate the integration and do not claim
production detector quality.

### Run the example

The repository includes the complete example under
[`examples/neural-observability-detector`](https://github.com/penserai/acteon/tree/main/examples/neural-observability-detector).
Run it from the repository root:

```bash
examples/neural-observability-detector/scripts/run.sh
```

The script starts Kafka, builds the CPU-only Laya service, downloads the pinned
checkpoint on the first run, performs 16 real model calls across four trials,
injects a detector crash before its Kafka commit, exercises Acteon's rules and
incident chain, and writes JSON and Markdown reports.

```text
examples/neural-observability-detector/
├── README.md
├── docker-compose.yml
├── model.lock.json
├── laya/
│   └── Dockerfile
├── questions/
│   ├── metrics.json
│   ├── traces.json
│   ├── logs.json
│   └── fusion.json
├── rules/
│   └── verdict-routing.yaml
├── fixtures/
│   ├── baseline.json
│   ├── log-noise.json
│   ├── pool-exhaustion.json
│   └── ambiguous-regression.json
├── results/
│   └── latest.md
└── scripts/run.sh
```

The Rust runner lives at
`crates/simulation/examples/neural_observability_simulation.rs`. Laya remains a
replaceable inference container behind its typed HTTP API.

### Four deterministic trials

| Trial | Inputs | Expected result |
|---|---|---|
| Healthy baseline | normal metrics, traces, and logs | no provider side effect |
| Log-only noise | burst of error-looking log templates, normal metrics and traces | no incident; demonstrates cross-signal resistance |
| Pool exhaustion | the three correlated signals from the scenario | one incident chain, even when the window is replayed |
| Ambiguous regression | high latency, conflicting trace and log labels, one missing source | one bounded investigation-agent run; no remediation |

The fixtures use fixed timestamps and IDs. The runner publishes their source
features to separate metrics, traces, and logs topics, consumes the resulting
broker positions, and joins them into 60-second event-time windows with 15
seconds of allowed lateness. It atomically checkpoints active windows,
finalized-window keys, event IDs, source watermarks, ready outputs, and source
offsets. The simulation then terminates its consumers before committing,
restores the checkpoint, and proves that Kafka redelivery neither reopens a
window nor loses an output. A final checkpoint is fsynced and renamed before
the source offsets are committed; the run must end with zero consumer lag.
One additional metrics record is deliberately duplicated independently of the
restart. `model.lock.json` records the model identity, revision, runtime
versions, artifact manifest, and named contract digests. The runner also keeps
an idempotency ledger for admitted verdicts. That makes the simulation an
integration test of real transport, recovery, inference, contracts, and policy
rather than a misleading benchmark of production model quality.

### Measured result

The recorded CPU run passed all four expected policy outcomes. Laya correctly
separated the first-stage signal conditions. Its raw fusion choice selected
`downstream_timeout` for the healthy, noise, and ambiguous fixtures and
`db_pool_exhaustion` for the correlated incident, all at low confidence. The
deterministic corroboration gate suppressed the healthy/noise false positives
and admitted an incident only when all three typed signal decisions agreed.

| Measure | Result |
|---|---:|
| Expected policy outcomes | 4 / 4 |
| Kafka source records accepted | 12 |
| Kafka duplicates and redeliveries rejected | 6 |
| Restart redeliveries deduplicated | 5 |
| Atomic checkpoint generations | 2 |
| Final Kafka consumer lag | 0 |
| Governed runtime packages / artifacts / question sets | 6 / 5 / 4 |
| Event-time windows | 4 |
| Real Laya calls | 16 |
| Total inference | 44,006 ms |
| Per-call p50 / p95 | 1,420 ms / 7,534 ms |
| Incident chains | 1 |
| Bounded investigator calls | 1 |
| Duplicate incident dispatches prevented | 1 |

See the
[`latest.md`](https://github.com/penserai/acteon/blob/main/examples/neural-observability-detector/results/latest.md)
report for per-signal decisions and confidence values. Laya reported an invalid
shipped temperature entry for part of this question shape, so the example
treats confidence as an observed score rather than a calibrated probability.

### Self-hosted Laya container

Run the upstream `laya-serve` application with only the `typed-decisions`
checkpoint loaded. It exposes Laya's Jev-compatible `POST /v1/systemone`
endpoint, so the simulation calls the model's native typed interface rather
than placing a generative model behind a classifier-shaped wrapper.

A reproducible CPU image is:

```dockerfile
FROM python:3.12-slim

ARG LAYA_VERSION=0.3.23
ARG TORCH_VERSION=2.14.0

RUN pip install --no-cache-dir "torch==${TORCH_VERSION}" \
      --index-url https://download.pytorch.org/whl/cpu \
    && pip install --no-cache-dir \
      "transformers==5.18.0" \
      "huggingface-hub==1.33.0" \
      "safetensors==0.8.0" \
      "numpy==2.5.3" \
    && pip install --no-cache-dir "laya[serve]==${LAYA_VERSION}"

COPY verify_and_serve.py /opt/acteon/verify_and_serve.py

ENTRYPOINT ["python", "/opt/acteon/verify_and_serve.py"]
```

The explicit dependencies preserve the tested environment and the PyTorch CPU
index prevents a Linux build from pulling CUDA packages. The Compose service
binds only to loopback, keeps one checkpoint resident, and caps concurrency:

```yaml
services:
  laya:
    build:
      context: ./laya
      args:
        LAYA_VERSION: "<pinned-version>"
    environment:
      LAYA_DEVICE: cpu
      LAYA_REPOSITORY: convaiinnovations/laya
      LAYA_MODELS: typed-decisions
      LAYA_DEFAULT_MODEL: typed-decisions
      LAYA_PRELOAD: "1"
      LAYA_MAX_LOADED: "1"
      LAYA_MAX_CONCURRENT: "4"
      LAYA_MAX_TOKEN_BUDGET: "1024"
      LAYA_REVISION: 55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851
      LAYA_API_KEY: "${LAYA_API_KEY:-acteon-laya-demo}"
      OMP_NUM_THREADS: "4"
      LAYA_THREADS: "4"
    ports:
      - "127.0.0.1:8000:8000"
    volumes:
      - laya-model-cache:/root/.cache/huggingface
      - ./model.lock.json:/opt/acteon/model.lock.json:ro
    healthcheck:
      test: ["CMD", "python", "-c", "import urllib.request; urllib.request.urlopen('http://localhost:8000/health')"]
      interval: 10s
      timeout: 3s
      retries: 12
    deploy:
      resources:
        limits:
          cpus: "4"
          memory: 8G

volumes:
  laya-model-cache:
```

The upstream CPU quickstart recommends allowing 8 GB of RAM and 10 GB of disk,
so call this a bounded single-model container rather than a tiny image. The
measured arm64 image was 1.57 GB, excluding the model cache. Persist the Hugging
Face cache to avoid downloading weights at every start. Replace the loopback
demo key with an injected secret outside this local example. After the artifacts
and their digests have been verified, the demo can run with `HF_HUB_OFFLINE=1`.
Laya's ONNX and per-channel INT8 path is a useful fast follow for a smaller CPU
deployment, after parity tests establish acceptable output drift.

### Typed observability request

Each question file contains the actual typed questions sent to Laya. For
example, `questions/metrics.json` produces this request:

```json
{
  "state": [
    {"field": "context", "value": "checkout-api has severe user impact. Database pool utilization is saturated, latency is ten times baseline, and errors are high."},
    {"field": "latency_p95_ms", "value": 1800},
    {"field": "latency_baseline_ms", "value": 180},
    {"field": "error_rate", "value": 0.084},
    {"field": "request_rate_per_second", "value": 126},
    {"field": "request_rate_baseline", "value": 120},
    {"field": "db_pool_utilization", "value": 0.96},
    {"field": "evidence_refs", "value": ["metrics:4:8821"]}
  ],
  "questions": {
    "pool_exhausted": {
      "type": "noul",
      "instructions": "Is database connection-pool exhaustion occurring?"
    },
    "condition": {
      "type": "choice",
      "instructions": "Choose the primary operational condition in this metrics window.",
      "criteria": {
        "healthy": "All service and dependency measurements are within their normal ranges.",
        "db_pool_pressure": "Database connection-pool usage is saturated and requests wait for connections.",
        "traffic_spike": "Request volume increased without evidence of database pool saturation.",
        "application_errors": "Application failures increased without a saturated dependency resource."
      }
    },
    "impact": {
      "type": "score",
      "instructions": "Score the operational impact of this metrics window.",
      "criteria": ["none", "low", "moderate", "high", "critical"]
    }
  },
  "model": "typed-decisions"
}
```

The runner renders each typed feature object into this schema-ordered array
before inference. JSON object key order is not semantic and serializers may
change it; the explicit array prevents serialization order from changing the
model input.

Laya returns a selected choice and its probability distribution, an expected
score with per-level probabilities, and `noul` as the probability of yes. It
also returns answer confidence, usage, and routing metadata. The four question
files are immutable inputs to the simulation and their SHA-256 digests are
recorded in `model.lock.json`.

The runner records `answer_confidence`, but the incident path requires the
declared metrics, trace, log, and fusion labels to agree. Production thresholds
still require calibration on held-out observability windows.

### Real Laya invocations

The runner calls Laya directly and validates every response into a closed local
type before using it:

```python
async def evaluate_laya(
    http: httpx.AsyncClient,
    state: dict,
    questions: dict,
) -> LayaResponse:
    response = await http.post(
        "http://laya:8000/v1/systemone",
        headers={"Authorization": f"Bearer {settings.laya_api_key}"},
        json={
            "state": state,
            "questions": questions,
            "model": "typed-decisions",
        },
        timeout=90.0,
    )
    response.raise_for_status()
    return LayaResponse.model_validate(response.json(), strict=True)
```

`LayaResponse` rejects extra fields, unknown question names or choice labels,
non-finite probabilities, values outside `0..1`, and unexpected routing model
identities. The runner also records `X-Inference-Time-Ms` and captures the
loaded model, revision, and device from Laya's health response when the
simulation starts.

The same call is visible without the runner, which is useful when proving that
the container is performing inference:

```bash
jq -n \
  --slurpfile fixture fixtures/pool-exhaustion.json \
  --slurpfile questions questions/metrics.json \
  '{state: ($fixture[0].metrics | to_entries | map({field: .key, value: .value})), questions: $questions[0], model: "typed-decisions"}' \
| curl --fail-with-body --silent \
  --header "Authorization: Bearer ${LAYA_API_KEY}" \
  --header "Content-Type: application/json" \
  --data-binary @- \
  http://localhost:8000/v1/systemone | jq .
```

The response must contain `answers.pool_exhausted.noul`,
`answers.condition.choice` and its `probabilities`,
`answers.impact.score` and its per-level `probabilities`, `usage`, and a
`routing.model` value of `typed-decisions`. The script saves that unedited JSON
inside `results/latest.json` instead of substituting hand-authored detector
values.

For every complete window, the runner makes the three signal calls concurrently
and then makes a fourth call whose state contains only their typed results:

```python
metrics, traces, logs = await asyncio.gather(
    laya.evaluate(metric_state, questions.metrics),
    laya.evaluate(trace_state, questions.traces),
    laya.evaluate(log_state, questions.logs),
)
verdict = await laya.evaluate(
    fusion_state(metrics, traces, logs),
    questions.fusion,
)
await acteon.dispatch(to_action(verdict))
```

These are four real requests to the local Laya model: three concurrently
submitted signal decisions followed by one fusion decision. A single CPU Laya
server executes its synchronous forward passes one at a time, so concurrency
overlaps client and HTTP work but does not make the model execute three passes
simultaneously. The JSON report embeds every raw response, including usage,
typed answers, confidence, and routing decisions, along with measured inference
latency and the server's model identity. `model.lock.json` separately records
the runtime versions, pinned revision, artifact manifest, and named contract
digests.

### Simulation sequence

1. Start the pinned Laya service and confirm the checkpoint revision and device
   through its authenticated health response.
2. Publish the fixed telemetry envelopes to separate Kafka topics, including
   one deliberate duplicate event ID.
3. Consume the three streams and correlate them into 60-second event-time
   windows, preserving broker positions and availability masks.
4. Invoke the three signal question sets concurrently over HTTP.
5. Validate the response IDs, types, labels, numeric ranges, probability sums,
   routing identity, and zero output-token count.
6. Send only the three typed results and availability mask to the fusion
   question set.
7. Apply the corroboration policy and dispatch `detector.verdict` through the
   real Acteon gateway and rules.
8. Verify suppression, the completed incident chain, or the investigator
   recording provider, then replay the incident key.
9. Generate JSON and Markdown reports with actual and expected outcomes.

### Assertions

The simulation passes only if it can show:

- each complete window produces three signal evaluations and one fusion
  evaluation carrying the pinned open-source model revision;
- malformed response IDs, types, labels, probabilities, routing metadata, or
  output-token counts are rejected before dispatch;
- a missing source increases uncertainty without inventing evidence;
- log-only noise does not create an incident;
- correlated pool exhaustion starts exactly one response chain;
- the ambiguous case reaches exactly one bounded investigator provider;
- replay produces the same verdict and no duplicate side effect;
- every side effect can be traced to source offsets and a model version;
- no low-confidence or invalid result reaches PagerDuty, Slack, or remediation.

### Failure injection

After the happy path works, use the simulation harness and recording providers
to exercise:

- one detector timing out;
- the Laya service being unavailable or returning the wrong response type;
- malformed or out-of-range model output;
- Kafka redelivery and consumer restart;
- a late trace record after the window closes;
- the notification provider failing and its circuit breaker opening;
- the agent provider reaching its concurrency quota;
- an unapproved model version attempting to publish a verdict.

These tests matter more than adding more incident classes to the first demo.

## Production rollout

Use staged enablement:

1. **Offline replay:** tune contracts, windows, and calibration on labeled
   history.
2. **Shadow:** consume live Kafka data and audit verdicts without side effects.
3. **Notify:** allow watch-band notifications with aggressive deduplication.
4. **Open incidents:** enable high-confidence incident chains.
5. **Agent assist:** enable the ambiguous, high-impact branch with read-only
   tools and a strict budget.
6. **Reversible automation:** allow a narrow remediation behind rate limits and
   rollback checks.
7. **Approval-gated changes:** keep risky or destructive operations behind a
   human decision.

At every stage, compare the new path with the previous one using identical
incident labels and cost accounting. The detector is ready to advance only
when it improves time-to-detect or operator load without exceeding the agreed
false-positive and agent-escalation budgets.

## Related documentation

- [Agentic Bus](../concepts/agentic-bus.md) — Kafka-backed topics,
  subscriptions, schemas, and typed envelopes
- [Task Chains](../features/chains.md) — multi-step response workflows
- [Parallel Steps](../features/parallel-steps.md) — fan-out/fan-in for model or
  provider calls
- [Ambient Swarm Provider](../features/swarm-provider.md) — bounded asynchronous
  agent runs
- [Simulation & Testing](../examples/simulation.md) — recording providers,
  failure injection, and assertions
- [Rule System](../concepts/rules.md) — deterministic policy before side effects
- [Laya](https://github.com/NandhaKishorM/laya) — open-source System One model
  and server
- [Laya HTTP API](https://nandhakishorm.github.io/laya/http-api/) — typed request,
  response, confidence, and routing contract
- [Laya Docker quickstart](https://nandhakishorm.github.io/laya/docker/) — CPU,
  cache, integrity, and serving configuration
