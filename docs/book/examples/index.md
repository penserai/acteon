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

The [cascading observability detector](../guides/neural-observability-detector.md)
uses real Kafka, Redis, and local Laya inference. Four scenarios cover normal
traffic, isolated noise, correlated failure, and incomplete evidence. Its output
includes model decisions, policy routes, durable recovery, quarantine repair,
and audited stage controls.

Operational effects are recorded by the example's providers so you can inspect
them locally. The model requests and transport recovery are real. See the guide
for the exact setup and the scope of each measurement.

## Test the boundary that matters

Use [rule tests](../features/rule-testing-cli.md) for deterministic policy,
[dry runs](../features/dry-run.md) for dispatch evaluation, and simulations for
execution and recovery. Check behavior after a worker stops, a receipt response
is lost, a provider fails, or an operator changes a stage's control state.
