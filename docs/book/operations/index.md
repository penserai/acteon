# Operate Acteon

Run automation with a clear view of its authority, progress, and recovery state.
Acteon's operational surface spans the Admin UI, HTTP API, CLI, audit storage,
execution history, and telemetry.

## Put the platform into service

1. Choose the [server features and integrations](../getting-started/installation.md)
   your workload uses.
2. Configure [state and audit backends](../backends/index.md), then apply the
   [required migrations](../reference/deployment.md).
3. Set up [authentication and grants](../api/authentication.md), rules, and
   approval policy.
4. Connect providers, model endpoints, worker processes, and Kafka where needed.
5. Establish retention, monitoring, and the recovery procedures for your execution model.

The [deployment guide](../reference/deployment.md) covers runtime setup. The
[configuration reference](../getting-started/configuration.md) documents settings.

## Find the evidence for the work

| Question | Where to look |
|---|---|
| What happened to an action? | [Audit trail](../features/audit-trail.md), dispatch outcomes, and durable receipts |
| Where is a process waiting? | [Execution history](../features/durable-executions.md), chain/workflow status, and worker queues |
| Which integration is failing? | [Provider health](../features/provider-health.md), [circuit breakers](../features/circuit-breaker.md), and tracing |
| Is stream processing making progress? | [Stage metrics and controls](../features/managed-stream-stages.md), Kafka lag, quarantine, and output backlog |
| What needs a person? | [Approvals](../features/approvals.md), retained dead letters, and reconciliation-required receipts |
| How does the service behave over time? | [Analytics](../features/analytics.md), [Grafana](../features/grafana-dashboards.md), and [Prometheus alerts](../features/prometheus-alerting.md) |

Use the [Admin UI](../admin-ui/index.md) for supported visual operations and the
[CLI](../api/cli.md) or [API](../api/index.md) for scripted and advanced operations.

## Recover at the correct boundary

An execution retry, a quarantine repair, an action replay, and a dispatch
reconciliation each resolve a different kind of failure. Start with the stored
outcome and the component's documented protocol. Use stable request identities
and observed revisions where required.

[Managed stage halt/resume](../features/managed-stream-stages.md) stops input
processing and replay claims; already-durable output delivery remains independent.
[Agent lifecycle controls](../features/operator-lifecycle.md) and swarm cancellation
address agent activity. [Durable dispatch](../features/durable-dispatch.md)
provides explicit reconciliation when an external effect is uncertain.

Plan [data retention](../features/data-retention.md),
[dead-letter retention](../features/dlq-retention.md), and component-specific audit
limits with these recovery procedures in mind.
