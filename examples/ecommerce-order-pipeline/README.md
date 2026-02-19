# E-Commerce Order Processing Pipeline

An end-to-end example exercising 15 Acteon features through an e-commerce order processing scenario. Orders flow from a storefront through Acteon, which enforces fraud screening, business-hours routing, rate limiting, and deduplication, then routes approved orders through a multi-step processing chain with lifecycle tracking and real-time streaming.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **Chains** | `order-processing` chain: validate → charge → fulfill → notify-customer (TOML config) |
| 2 | **Sub-chains** | `fraud-review` sub-chain invoked by order-processing chain step |
| 3 | **Conditional branching** | Branch on `body.logged` after validate step |
| 4 | **Deny** | Block orders from sanctioned regions (NK, SY, IR) — rule priority 1 |
| 5 | **Schedule** | After-hours orders delayed 30s — rule priority 10 |
| 6 | **Time-based conditions** | `time.hour`, `time.weekday_num` with `US/Eastern` timezone |
| 7 | **Request approval** | Orders > $500 require manager approval (60s timeout) — priority 3 |
| 8 | **State machine** | Order lifecycle: placed → confirmed → shipped → delivered (API-driven) |
| 9 | **Throttle** | Max 10 orders/minute per merchant — rule priority 20 |
| 10 | **Dedup** | Same dedup_key deduplicated within 5 min — rule priority 15 |
| 11 | **Modify** | Chain payload templates enrich each step with order metadata |
| 12 | **Quotas** | 200 orders/hour per merchant via `POST /v1/quotas` |
| 13 | **Data retention** | Audit 14 days, events 3 days via `POST /v1/retention` |
| 14 | **Audit + redaction** | Full audit trail with `card_last4`, `billing_zip` redacted |
| 15 | **Payload templates** | Order confirmation + fraud alert emails rendered from MiniJinja templates |

## Prerequisites

- PostgreSQL (for durable state + audit)
- `jq` (for script output formatting)

## Quick Start

```bash
# 1. Start PostgreSQL
docker compose --profile postgres up -d

# 2. Run database migrations
scripts/migrate.sh -c examples/ecommerce-order-pipeline/acteon.toml

# 3. Start Acteon
cargo run -p acteon-server --features postgres -- \
  -c examples/ecommerce-order-pipeline/acteon.toml

# 4. Setup API resources (quotas, retention)
cd examples/ecommerce-order-pipeline
bash scripts/setup.sh

# 5. Send 15 sample orders
bash scripts/send-orders.sh

# 6. Manage order lifecycle (state transitions)
bash scripts/manage-order.sh

# 7. SSE streaming demo (optional, in separate terminal)
bash scripts/stream-orders.sh

# 8. View comprehensive report
bash scripts/show-report.sh

# 9. Cleanup API resources
bash scripts/teardown.sh
```

## File Structure

```
ecommerce-order-pipeline/
├── acteon.toml              # Server config (chains, circuit breakers, background, audit)
├── rules/
│   ├── fraud.yaml           # Deny sanctioned, approve high-value
│   ├── processing.yaml      # Schedule after-hours, dedup, throttle
│   └── safety.yaml          # Catch-all suppress
├── scripts/
│   ├── setup.sh             # Create quotas, retention, templates + profiles via API
│   ├── send-orders.sh       # Fire 15 sample orders exercising all features
│   ├── manage-order.sh      # Transition orders through lifecycle via API
│   ├── stream-orders.sh     # SSE streaming demo (real-time order events)
│   ├── show-report.sh       # Query audit/chains/events/quotas/retention summary
│   └── teardown.sh          # Clean up API-created resources
└── README.md
```

## Architecture

```
                    ┌─────────────────────────┐
  Storefront ──────►│    Acteon Gateway        │
  (Web, Mobile,     │                         │
   API clients)     │  Rules Engine           │
                    │  ┌─deny sanctioned───┐  │     ┌──────────────────┐
                    │  ├─approve >$500─────┤  │────►│ payment-gateway  │──┐ fallback
                    │  ├─schedule after-hrs─┤  │     ├──────────────────┤  │
                    │  ├─dedup 5min────────┤  │────►│ warehouse        │  │
                    │  └─throttle 10/min───┘  │     ├──────────────────┤  │
                    │                         │────►│ email-service    │  │
                    │  Chain Engine           │     ├──────────────────┤  │
                    │  ┌─validate───────────┐ │     │ fraud-analyzer   │  │
                    │  ├─fraud-review (sub)─┤ │     ├──────────────────┤  │
                    │  ├─charge─────────────┤ │     │ compliance-queue │◄─┘
                    │  ├─fulfill────────────┤ │     └──────────────────┘
                    │  └─notify-customer────┘ │
                    │                         │     ┌──────────────────┐
                    │  Background Jobs        │────►│ SSE /v1/stream   │
                    │  ├─scheduled actions   │     │ (real-time feed) │
                    │  └─retention reaper    │     └──────────────────┘
                    └─────────────────────────┘
```

## Rule Evaluation

Acteon uses **first-match-wins** rule evaluation: rules are sorted by priority (lowest number first) and the first matching rule determines the action's outcome.

```
Priority 1:  deny-sanctioned-regions   → Suppressed (hard deny)
Priority 3:  approve-high-value        → PendingApproval
Priority 10: schedule-after-hours      → Scheduled (30s delay)
Priority 15: dedup-double-submit       → Executed (new) or Deduplicated (seen)
Priority 20: throttle-merchant-orders  → Executed (under limit) or Throttled
Priority 100: deny-unmatched           → Suppressed (catch-all)
```

Each order matches exactly one rule. More specific conditions (sanctioned regions, high-value) are evaluated first to ensure security gates fire before general processing.

## Chain Flow

The `order-processing` chain handles orders through 4 steps, with an optional sub-chain for fraud review:

```
validate (payment-gateway)
    │
    ├─ body.logged == true ──► charge (payment-gateway)
    │                              │
    │                              └──► fulfill (warehouse)
    │                                       │
    │                                       └──► notify-customer (email-service)
    │
    └─ (no branch match) ──► fraud-review-step (sub-chain)
                                  ├─ analyze (fraud-analyzer)
                                  ├─ flag-compliance (compliance-queue)
                                  └─ notify-merchant (email-service)
```

## Expected Outcomes from `send-orders.sh`

| Orders | Count | Expected Outcome |
|--------|-------|-----------------|
| Standard ($50-$200) | 3 | `Executed` (pass dedup as new, then throttle gate) |
| High-value (>$500) | 2 | `PendingApproval` (requires manager approval) |
| Sanctioned region | 2 | `Suppressed` (denied by fraud rule) |
| Duplicate | 2 | 1st `Executed`, 2nd `Deduplicated` |
| Rapid-fire | 3 | `Executed` (pass dedup as new, under throttle limit) |
| After-hours | 3 | `Scheduled` (if run outside 9-17 ET Mon-Fri) |

## Business Hours Demo

The `schedule-after-hours` rule uses time-based conditions with the `US/Eastern` timezone:
- Orders placed before 9 AM or after 5 PM ET are delayed 30 seconds
- Weekend orders (Saturday/Sunday) are also delayed
- Run `send-orders.sh` outside business hours to see `Scheduled` outcomes

## Audit Redaction

All order payloads containing `card_last4`, `billing_zip`, or `card_number` fields are automatically redacted to `[REDACTED]` in the audit trail. Run `show-report.sh` to verify redacted fields appear in the audit.

## Notes

- **Log providers** return `{"provider": "<name>", "logged": true}`. Chain branching uses `body.logged == true` as a condition field.
- **Approval timeout** is 60 seconds. The approval URL appears in server logs when a high-value order is held.
- **Quota window** uses `"hourly"` string format. Other options: `"daily"`, `"weekly"`, `"monthly"`.
- **State machine** transitions are driven by API calls (`PUT /v1/events/{fingerprint}/transition`), not by rules. The `manage-order.sh` script demonstrates the full lifecycle.
- **First-match-wins**: Acteon evaluates rules by priority and stops at the first match. In this example, dedup (priority 15) catches all standard orders before throttle (priority 20). To see throttle fire, send >10 orders within 60 seconds after restarting the server (to clear dedup state).
- **Payload templates** render order confirmations and fraud alerts from MiniJinja templates. Standard orders dispatch with `template: "order-confirmation"`, composing inline subjects with HTML body templates via `$ref`. Templates are created in `setup.sh` and cleaned up in `teardown.sh`.
