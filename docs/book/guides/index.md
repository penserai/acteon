# Build a complete flow

See how Acteon's execution and governance capabilities work together in a real
system. Each guide connects a domain problem to policy, integrations, execution,
and the operational evidence needed to run it.

## Agents and intelligent operations

### [Cascading observability with local neural models](neural-observability-detector.md)

Join metrics, traces, and logs from Kafka, call a real local Laya model, and let
deterministic policy choose suppression, an incident chain, or bounded
investigation. The runnable simulation covers checkpoints, quarantine repair,
audited stage halt/resume, and delivery recovery, with measured results.

### [A2A and agent registry](a2a-agent-registry-tutorial.md)

Register and discover agents, follow interoperable tasks, and connect agent
communication to Acteon's scoped operational interfaces.

### [Agent swarm coordination](agent-swarm-coordination.md)

Coordinate specialist agents with identity, permissions, approvals, and usage
limits. Explore evaluation, cross-engine adversarial critique, and recovery in
a multi-agent execution loop.

## Business and infrastructure operations

| Guide | Build |
|---|---|
| [Incident response](incident-response-pipeline.md) | Alert triage, deduplication, escalation chains, war-room sub-chains, and event lifecycle tracking |
| [E-commerce orders](ecommerce-order-pipeline.md) | Fraud screening, approval gates, scheduled work, fulfillment chains, and payment-field redaction |
| [Healthcare notifications](healthcare-notification-pipeline.md) | Notification policy, sensitive-data routing, approval workflows, and tamper-evident audit configuration |
| [AWS event pipeline](aws-event-pipeline.md) | Sensor-event routing to cloud services, chains, fallbacks, and event grouping with a LocalStack setup |

## Bring an existing system

- [Migrate from Alertmanager](migrating-from-alertmanager.md): map routing,
  inhibition, time intervals, and silences onto Acteon.
- [Migrate to the Agentic Bus](agentic-bus-migration.md): adopt topics,
  subscriptions, agent identity, and conversations.

For focused code samples, use [examples and simulation](../examples/index.md).
For a first local request, use the [quick start](../getting-started/quickstart.md).
