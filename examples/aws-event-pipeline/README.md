# AWS Event-Driven Pipeline

An end-to-end example exercising all 4 AWS providers (SNS, Lambda, EventBridge, SQS) through an IoT smart-building telemetry scenario. Sensor devices publish temperature, humidity, motion, and energy readings that Acteon routes to AWS services via LocalStack, using DynamoDB for state and audit storage.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **AWS SNS** | Critical temperature and intrusion alerts fan out via SNS topic |
| 2 | **AWS Lambda** | Telemetry normalization and anomaly detection via Lambda functions |
| 3 | **AWS EventBridge** | Device lifecycle events published to custom event bus |
| 4 | **AWS SQS** | Metrics queued for batch processing; dead-letter queue for failures |
| 5 | **DynamoDB** | State + audit backends via LocalStack |
| 6 | **Chains** | `telemetry-processing`: normalize → detect → publish → archive |
| 7 | **Sub-chains** | Critical alert path invokes `critical-alert` sub-chain (SNS → EventBridge) |
| 8 | **Conditional branching** | Branch on normalization success and anomaly detection result |
| 9 | **Circuit breakers + fallback** | Lambda → DLQ; SNS → local-fallback |
| 10 | **Recurring actions** | Device heartbeat check every 5 minutes |
| 11 | **Data retention** | Audit 7 days, events 2 days |
| 12 | **Event grouping** | Humidity readings batched by floor, 30s window |
| 13 | **Quotas** | 500/hour for smartbuilding-hq tenant |
| 14 | **Throttle** | Telemetry ingestion: 30/min per tenant |
| 15 | **Dedup** | Duplicate sensor readings deduplicated within 60s |
| 16 | **Suppress** | Block telemetry from test devices |
| 17 | **Deny** | Reject unsigned firmware updates |
| 18 | **Schedule** | Energy reports delayed 60s for batch aggregation |
| 19 | **Modify** | Enrich telemetry with pipeline version and building zone |
| 20 | **Audit + redaction** | Full audit with `aws_role_arn`/`function_arn`/`queue_url` redacted |

## Prerequisites

- [LocalStack](https://localstack.cloud/) (provides SNS, Lambda, EventBridge, SQS, DynamoDB)
- `awslocal` CLI (`pip install awscli-local`)
- `jq` (for script output formatting)

## Quick Start

```bash
# 1. Start LocalStack
docker run --rm -d --name localstack -p 4566:4566 localstack/localstack

# 2. Create AWS resources (SNS topic, Lambda functions, EventBridge bus, SQS queues, DynamoDB tables)
bash examples/aws-event-pipeline/scripts/setup.sh

# 3. Start Acteon with DynamoDB backend
cargo run -p acteon-server --features dynamodb -- \
  -c examples/aws-event-pipeline/acteon.toml

# 4. Create API resources (quota, retention, recurring heartbeat)
bash examples/aws-event-pipeline/scripts/setup-api.sh

# 5. Fire ~21 sample telemetry events
bash examples/aws-event-pipeline/scripts/send-telemetry.sh

# 6. View comprehensive report
bash examples/aws-event-pipeline/scripts/show-report.sh

# 7. Cleanup API resources
bash examples/aws-event-pipeline/scripts/teardown.sh

# 8. Stop LocalStack
docker stop localstack
```

## File Structure

```
aws-event-pipeline/
├── acteon.toml              # Server config (AWS providers, chains, circuit breakers, DynamoDB)
├── rules/
│   ├── routing.yaml         # Route to SNS/Lambda/EventBridge/SQS, enrich metadata, allow
│   ├── processing.yaml      # Dedup, throttle, group humidity, schedule energy
│   └── safety.yaml          # Suppress test devices, deny unsigned firmware, catch-all
├── scripts/
│   ├── setup.sh             # Create LocalStack resources (AWS + DynamoDB)
│   ├── setup-api.sh         # Create quotas + retention + recurring via API
│   ├── send-telemetry.sh    # Fire ~21 sample telemetry events
│   ├── show-report.sh       # Query audit/chains/events/health/quotas/groups
│   └── teardown.sh          # Clean up API-created resources
└── README.md
```

## Architecture

```
                    ┌─────────────────────────┐
  IoT Sensors ─────►│    Acteon Gateway        │
  (temperature,     │                         │
   humidity,        │  Rules Engine           │        ┌──────────────────┐
   motion,          │  ┌─suppress test──────┐ │       │ SNS              │
   energy)          │  ├─deny unsigned fw───┤ │──────►│ building-alerts  │
                    │  ├─dedup 60s──────────┤ │       ├──────────────────┤
                    │  ├─throttle 30/min────┤ │       │ Lambda           │
                    │  ├─group humidity─────┤ │──────►│ anomaly-detector │
                    │  ├─schedule energy────┤ │       │ normalizer       │
                    │  ├─reroute critical───┤ │       ├──────────────────┤
                    │  ├─chain readings─────┤ │       │ EventBridge      │
                    │  └─allow remaining────┘ │──────►│ building-events  │
                    │                         │       ├──────────────────┤
                    │  Chain Engine           │       │ SQS              │
                    │  ┌─normalize──────────┐ │──────►│ telemetry-metrics│
                    │  ├─detect-anomaly────┤ │       │ telemetry-dlq    │
                    │  ├─publish (sub)─────┤ │       ├──────────────────┤
                    │  └─archive-metrics───┘ │       │ Log              │
                    │                         │──────►│ local-fallback   │
                    │  Background Jobs        │       └──────────────────┘
                    │  ├─group flush         │
                    │  ├─recurring heartbeat │       ┌──────────────────┐
                    │  └─retention reaper    │       │ DynamoDB         │
                    │                         │──────►│ acteon_state     │
                    └─────────────────────────┘       │ acteon_audit     │
                                                      └──────────────────┘
```

## Chain Flow

The `telemetry-processing` chain handles normal sensor readings:

```
normalize (telemetry-normalizer Lambda)
    │
    ├─ body.logged == true ──► detect-anomaly (anomaly-detector Lambda)
    │                              │
    │                              ├─ success == true ──► publish-event (sub-chain)
    │                              │                          ├─ fan-out-alert (SNS)
    │                              │                          └─ publish-to-bus (EventBridge)
    │                              │
    │                              └─ (default) ──► archive-metrics (SQS)
    │
    └─ (no branch match) ──► detect-anomaly (next step)
```

## Rule Evaluation Order

Rules are evaluated by priority (lowest number = highest priority):

| Priority | Rule | File | Action |
|----------|------|------|--------|
| 1 | `suppress-test-devices` | safety.yaml | Suppress |
| 1 | `deny-unsigned-firmware` | safety.yaml | Deny |
| 2 | `dedup-sensor-readings` | processing.yaml | Deduplicate 60s |
| 3 | `throttle-telemetry` | processing.yaml | Throttle 30/min |
| 4 | `group-humidity-readings` | processing.yaml | Group by floor, 30s |
| 4 | `schedule-energy-reports` | processing.yaml | Schedule 60s delay |
| 5 | `critical-temperature-alert` | routing.yaml | Reroute → SNS |
| 6 | `intrusion-alert` | routing.yaml | Reroute → SNS |
| 10 | `chain-sensor-readings` | routing.yaml | Chain → telemetry-processing |
| 12 | `lifecycle-to-eventbridge` | routing.yaml | Reroute → EventBridge |
| 14 | `firmware-to-sqs` | routing.yaml | Reroute → SQS |
| 16 | `enrich-telemetry-metadata` | routing.yaml | Modify metadata |
| 20 | `allow-remaining` | routing.yaml | Allow |
| 100 | `deny-unmatched` | safety.yaml | Suppress (catch-all) |

## Expected Outcomes from `send-telemetry.sh`

| Events | Count | Expected Outcome |
|--------|-------|-----------------|
| Critical temp | 2 | `rerouted` to SNS (value > 85) |
| Intrusion | 2 | `rerouted` to SNS (motion intrusion) |
| Normal temp | 3 | `chain_started` (telemetry-processing) |
| Humidity | 3 | `grouped` (batched by floor, 30s) |
| Energy | 2 | `scheduled` (60s delay) |
| Rapid-fire | 3 | Some `throttled` (if >30/min reached) |
| Duplicate | 2 | 1 `deduplicated` (same dedup_key) |
| Test device | 2 | `suppressed` (environment=test) |
| Lifecycle | 1 | `rerouted` to EventBridge |
| Unsigned FW | 1 | `denied` (signed=false) |

## Circuit Breaker Demo

Two circuit breaker fallback paths are configured:

- **anomaly-detector** (Lambda) trips after 2 failures → falls back to `dead-letter-queue` (SQS)
- **alert-fanout** (SNS) trips after 2 failures → falls back to `local-fallback` (log)

If LocalStack is not running, the AWS providers will fail and circuit breakers will trip, demonstrating the fallback routing.

## Notes

- **LocalStack** provides a full local AWS environment. All AWS provider endpoints point to `http://localhost:4566`.
- **DynamoDB tables** (`acteon_state`, `acteon_audit`) are created by `setup.sh` with on-demand billing.
- **Lambda functions** use mock Python handlers that simulate anomaly detection and telemetry normalization.
- **Log provider** returns `{"provider": "<name>", "logged": true}`. Chain branching uses `body.logged == true` as a condition.
- **Quota window** uses `"hourly"` string format. Other options: `"daily"`, `"weekly"`, `"monthly"`.
- All audit payloads containing `aws_role_arn`, `function_arn`, or `queue_url` fields are automatically redacted.
