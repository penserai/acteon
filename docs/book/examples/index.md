# Examples and simulation

Try a small building block, then validate the failure and recovery behavior of
a complete flow. Acteon includes code examples, runnable domain setups, and a
simulation framework with recording providers and fault injection.

| Start here | What you will learn |
|---|---|
| [Basic usage](basic-usage.md) | Create actions, apply rules, deduplicate requests, and inspect outcomes |
| [Advanced patterns](advanced-patterns.md) | Combine provider routing, event lifecycles, inhibition, and deployment configuration |
| [Simulation and testing](simulation.md) | Exercise providers, failure modes, multi-node behavior, and backend combinations |
| [End-to-end guides](../guides/index.md) | Build agent, observability, incident, commerce, healthcare, and cloud flows |

## A complete, measurable example

The [ads measurement pipeline](../guides/ads-measurement-pipeline.md) turns a
synthetic CTV live-event workload into billing-grade measurements and operational
alerts. It exercises schema validation, two-layer deduplication, event-time
ordering, click attribution, idempotent ledger writes, reconciliation, severity
routing, and cross-node alert suppression in one local command.

The [cascading observability detector](../guides/neural-observability-detector.md)
uses real Kafka, Redis, and local Laya inference. Four scenarios cover normal
traffic, isolated noise, correlated failure, and incomplete evidence. Its output
includes model decisions, policy routes, durable recovery, quarantine repair,
and audited stage controls.

Operational effects are recorded by the example's providers so you can inspect
them locally. The model requests and transport recovery are real. See the guide
for the exact setup and the scope of each measurement.

The [bank transaction pipeline](../guides/bank-transaction-pipeline.md) sends
synthetic transactions through a suspicious-activity filter and records each
flagged transaction in the event timeline. It prints the results with cursor
pagination and demonstrates CEL searches for exact UUIDs, full text, and combined
attributes, plus account-group queries. It runs locally without external services.

## Test the boundary that matters

Use [rule tests](../features/rule-testing-cli.md) for deterministic policy,
[dry runs](../features/dry-run.md) for dispatch evaluation, and simulations for
execution and recovery. Check behavior after a worker stops, a receipt response
is lost, a provider fails, or an operator changes a stage's control state.
