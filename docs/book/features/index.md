# Platform capabilities

Acteon combines execution, governance, and recovery in a set of composable
building blocks. Choose a capability for the responsibility it owns, then
connect it to the rest of your flow.

## Execute work

| Capability | What it enables |
|---|---|
| [Action dispatch](../concepts/pipeline.md) and [providers](../concepts/providers.md) | Execute integration calls under configured policy |
| [Durable dispatch admission](durable-dispatch.md) | Accept work with an idempotent receipt and recover its recorded outcome |
| [Chains](chains.md), [sub-chains](sub-chains.md), and [parallel steps](parallel-steps.md) | Compose steps, data flow, branches, and parallel work |
| [Durable executions](durable-executions.md) and [chain retry](chain-retry.md) | Preserve progress, pin definitions, wait on timers/signals, and retry failed steps |
| [Task queues](task-queues.md) and [workflows as code](workflows.md) | Run custom code on your workers with checkpoints and continuations |
| [Scheduled](scheduled-actions.md) and [recurring actions](recurring-actions.md) | Run work later or on a recurring schedule |

[Choose your execution model](../concepts/execution-model.md).

## Govern behavior

| Capability | What it enables |
|---|---|
| [Authentication](../api/authentication.md) and [scoped grants](api-key-scoping.md) | Control which callers can operate within each namespace and tenant |
| [Agent workforce](workforce.md) | Organize humans and agents with explicit representation mandates and current relationship checks |
| [Execution permits](execution-permits.md) and [governance management](governance.md) | Bound qualified effects and intervene through current resource and participant controls |
| [Suppression](suppression.md), [deduplication](deduplication.md), and [throttling](throttling.md) | Block unwanted work, recognize duplicates, and bound dispatch rates |
| [Rerouting](rerouting.md), [payload modification](modification.md), and [templates](payload-templates.md) | Select integrations and shape requests before execution |
| [Human approvals](approvals.md) and [tenant quotas](tenant-quotas.md) | Require authorization for sensitive work and limit usage |
| [Time-based rules](time-based-rules.md), [silences](silences.md), and [time intervals](time-intervals.md) | Express operating windows and maintenance policy |
| [Action signing](action-signing.md), [encryption](payload-encryption.md), and [compliance mode](compliance-mode.md) | Verify origin and protect retained evidence |

[Understand the governance model](../concepts/governance.md).

## Coordinate agents and invoke models

Use [MCP](../api/mcp-server.md) to expose supported operations to agents,
[A2A](a2a.md) for interoperable agent tasks, and the [Agentic Bus](../concepts/agentic-bus.md)
for identity, inboxes, and conversations. The [swarm orchestrator](agent-swarm.md)
coordinates agent teams; the [swarm provider](swarm-provider.md) accepts prepared
goals through dispatch.

[Governed model calls](governed-model-provider.md), [LLM guardrails](llm-guardrails.md),
and [semantic routing](semantic-routing.md) give inference distinct roles in a
flow. [Agent lifecycle controls](operator-lifecycle.md) provide operator intervention.

[Explore agents and AI](agents-and-ai.md).

## Process events and streams

[Event grouping](event-grouping.md) and [state machines](state-machines.md) manage
event lifecycles. [Event-time windows](event-time-windows.md) correlate sources,
while [managed stream stages](managed-stream-stages.md) own typed processing and
durable progress. [Consume contracts and quarantine](stream-input-contracts.md)
handle invalid inputs; [checkpoints](stream-checkpoints.md),
[live Kafka acknowledgements](live-kafka-acknowledgements.md), and
[managed outbox delivery](managed-stream-outbox.md) connect processing to recovery.

[Explore streams and events](streams-and-events.md).

## Operate and improve

- **Inspect:** [Admin UI](../admin-ui/index.md), [audit trail](audit-trail.md), [analytics](analytics.md), and [SSE event streaming](event-streaming.md).
- **Observe:** [provider health](provider-health.md), [tracing](distributed-tracing.md), [Grafana dashboards](grafana-dashboards.md), and [Prometheus alerting](prometheus-alerting.md).
- **Recover:** [circuit breakers](circuit-breaker.md), [action replay](action-replay.md), [data retention](data-retention.md), and [dead-letter retention](dlq-retention.md).
- **Validate:** [dry runs](dry-run.md), [rule playground](rule-playground.md), [rule coverage](rule-coverage.md), [rule tests](rule-testing-cli.md), and [simulation](../examples/simulation.md).

## Connect your environment

Use [native providers](native-providers.md), [AWS](aws-providers.md),
[Azure](azure-providers.md), or [GCP](gcp-providers.md) integrations. Extend provider
behavior through the Rust [provider abstraction](../concepts/providers.md), or
add custom rule evaluation with [WASM plugins](wasm-plugins.md).
[Attachments](attachments.md) carry files alongside supported provider payloads.
