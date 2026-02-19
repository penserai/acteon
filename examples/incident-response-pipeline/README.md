# Incident Response Pipeline

An end-to-end example exercising 15 Acteon features through an ops incident response scenario. Alerts flow from monitoring systems through Acteon, which triages them via chain workflows, routes notifications to multiple providers, tracks alert lifecycle through event states, and protects against alert storms via throttling, dedup, and quotas.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **Chains** | `incident-triage` chain: classify → escalate → create-ticket |
| 2 | **Sub-chains** | Critical path invokes `war-room-setup` sub-chain (3 steps) |
| 3 | **Conditional branching** | Branch on `body.logged` after classify step |
| 4 | **Event state management** | Seed event, transition `open → ack → investigating → resolved` via API |
| 5 | **Circuit breakers + fallback** | PagerDuty → webhook-fallback; email-alerts → slack-alerts |
| 6 | **Recurring actions** | Health-check alert every 60s via `POST /v1/recurring` |
| 7 | **Data retention** | Audit 7 days, events 1 day via `POST /v1/retention` |
| 8 | **Event grouping** | Low-severity alerts batched 30s before notification |
| 9 | **Quotas** | 100/hour for ops-team via `POST /v1/quotas` |
| 10 | **Throttle** | Alert storm protection: 20/min per tenant |
| 11 | **Dedup** | Same fingerprint deduplicated within 5 min |
| 12 | **Suppress** | Block alerts from `test` environment |
| 13 | **Modify** | Enrich all alerts with `pipeline_version` metadata |
| 14 | **Audit + redaction** | Full audit with `api_key`/`webhook_url` redacted |
| 15 | **Payload templates** | Slack + email alert bodies rendered from MiniJinja templates via profiles |

## Prerequisites

- PostgreSQL (for durable state + audit)
- `jq` (for script output formatting)

## Quick Start

```bash
# 1. Start PostgreSQL
docker compose --profile postgres up -d

# 2. Run database migrations
scripts/migrate.sh -c examples/incident-response-pipeline/acteon.toml

# 3. Start Acteon
cargo run -p acteon-server --features postgres -- \
  -c examples/incident-response-pipeline/acteon.toml

# 4. Setup API resources (quotas, retention, recurring, templates)
cd examples/incident-response-pipeline
bash scripts/setup.sh

# 5. Fire 20 sample alerts
bash scripts/send-alerts.sh

# 6. Manage incident lifecycle (event state transitions)
bash scripts/manage-incident.sh

# 7. View comprehensive report
bash scripts/show-report.sh

# 8. Cleanup API resources
bash scripts/teardown.sh
```

## File Structure

```
incident-response-pipeline/
├── acteon.toml              # Server config (chains, circuit breakers, background, audit)
├── rules/
│   ├── triage.yaml          # Suppress test env, throttle storms, trigger chains
│   ├── routing.yaml         # Dedup, modify metadata, group low-severity, allow
│   └── safety.yaml          # Catch-all deny
├── scripts/
│   ├── setup.sh             # Create quotas + retention + recurring + templates via API
│   ├── send-alerts.sh       # Fire 20 sample alerts exercising all features
│   ├── manage-incident.sh   # Transition events through lifecycle via API
│   ├── show-report.sh       # Query audit/chains/events/health/quotas/groups
│   └── teardown.sh          # Clean up API-created resources
└── README.md
```

## Architecture

```
                    ┌─────────────────────┐
  Monitoring ──────►│   Acteon Gateway     │
  (Datadog,         │                     │
   Prometheus,      │  Rules Engine       │
   Grafana)         │  ┌─suppress test──┐ │
                    │  ├─throttle 20/m──┤ │     ┌──────────────┐
                    │  ├─dedup 5min─────┤ │────►│ slack-alerts  │
                    │  ├─modify meta────┤ │     ├──────────────┤
                    │  ├─group low──────┤ │────►│ pagerduty    │──┐ fallback
                    │  └─chain hi/crit──┘ │     ├──────────────┤  │
                    │                     │────►│ email-alerts  │  │
                    │  Chain Engine       │     ├──────────────┤  │
                    │  ┌─classify────────┐│     │ ticket-system│  │
                    │  ├─escalate────────┤│     ├──────────────┤  │
                    │  ├─war-room (sub)──┤│     │ webhook-fb   │◄─┘
                    │  └─create-ticket───┘│     └──────────────┘
                    │                     │
                    │  Background Jobs    │
                    │  ├─group flush     │
                    │  ├─recurring check │
                    │  └─retention reaper│
                    └─────────────────────┘
```

## Chain Flow

The `incident-triage` chain handles critical and high severity alerts:

```
classify (slack-alerts)
    │
    ├─ body.logged == true ──► escalate (pagerduty)
    │                              │
    │                              ├─ success == true ──► war-room (sub-chain)
    │                              │                          ├─ create-channel
    │                              │                          ├─ page-oncall
    │                              │                          └─ open-ticket
    │                              │
    │                              └─ (default) ──► create-ticket
    │
    └─ (no branch match) ──► escalate (next step)
```

## Expected Outcomes from `send-alerts.sh`

| Alerts | Count | Expected Outcome |
|--------|-------|-----------------|
| Critical | 2 | `chain_started` (full chain + sub-chain) |
| High | 3 | `chain_started` (chain, simpler path) |
| Low | 5 | `grouped` (batched by service, 30s) |
| Storm | 5 | Some `throttled` (if >20/min reached) |
| Duplicate | 3 | 2 `deduplicated` (same dedup_key) |
| Test-env | 2 | `suppressed` (environment=test) |

## Circuit Breaker Demo

The `webhook-fallback` provider intentionally targets `http://localhost:9999/fallback`, which will fail unless you run a local echo server. After 2 failures, the circuit breaker trips:

- **PagerDuty** circuit opens → falls back to `webhook-fallback`
- **email-alerts** circuit opens → falls back to `slack-alerts`

To see the webhook-fallback succeed, optionally run:
```bash
python3 -m http.server 9999
```

## Notes

- **Log providers** return `{"provider": "<name>", "logged": true}`. Chain branching uses `body.logged == true` as a condition field.
- **Event seeding** in `manage-incident.sh` dispatches an alert through the API to create initial event state.
- **Quota window** uses `"hourly"` string format. Other options: `"daily"`, `"weekly"`, `"monthly"`.
- All audit payloads containing `api_key`, `webhook_url`, or `pagerduty_key` fields are automatically redacted to `[REDACTED]`.
- **Payload templates** render Slack and email alert bodies from MiniJinja templates. The `slack-alert` and `email-alert` profiles compose inline subjects with `$ref` body templates. Templates are created in `setup.sh` and cleaned up in `teardown.sh`.
