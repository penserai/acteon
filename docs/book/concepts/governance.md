# Governance at the execution boundary

Automation becomes useful when it can change things. Governance makes those
changes manageable: who requested them, what policy applied, which execution
performed them, and what an operator can do next.

Acteon applies controls to work routed through its configured interfaces. An
agent's direct calls to external tools and a worker's arbitrary side effects
remain outside that boundary unless you integrate them with Acteon. Design the
execution path and credentials together so policy governs the operations that
matter.

## Establish authority

[Authentication](../api/authentication.md) identifies callers. Namespace and
tenant scopes, roles, and [API key grants](../features/api-key-scoping.md) limit
what they can do. [Agent lifecycle controls](../features/operator-lifecycle.md)
let operators suspend or ban registered agents. [Action signing](../features/action-signing.md)
adds verifiable request origin when configured.

Give an application, agent, and operator the permissions each needs. Reading
stage status, enqueueing a quarantine repair, and halting a stage are separate
permissions, for example.

## Decide what may execute

[Rules](rules.md) inspect actions and choose how they proceed: allow, suppress,
deduplicate, throttle, reroute, modify, or start configured orchestration.
[Tenant quotas](../features/tenant-quotas.md) bound usage, while
[human approvals](../features/approvals.md) put a decision in a person's hands.
Schedules, silences, and time intervals express when work is appropriate.

Use [dry runs](../features/dry-run.md), the [rule playground](../features/rule-playground.md),
and [rule tests](../features/rule-testing-cli.md) to inspect policy before deploying it.

## Bound model and agent behavior

A [governed model provider](../features/governed-model-provider.md) binds an
inference integration to a model lock, request material, and response schema.
It verifies the runtime identity it can observe and rejects invalid responses
before they enter a downstream step. [LLM guardrails](../features/llm-guardrails.md)
and [semantic routing](../features/semantic-routing.md) offer additional model-assisted
policy mechanisms.

A valid schema establishes the shape of an answer; it does not establish that
the answer is correct. Keep operational decisions explicit: corroborate evidence,
apply deterministic rules, bound investigation, and require approval where needed.
The [neural observability guide](../guides/neural-observability-detector.md) shows
this division of responsibility with real model outputs.

## Preserve evidence and enable intervention

| Need | Capability |
|---|---|
| Explain an action outcome | [Audit trail](../features/audit-trail.md) and [rule evaluation](../features/rule-playground.md) |
| Follow long-running work | [Execution histories](../features/durable-executions.md), [Admin UI](../admin-ui/index.md), and [event streaming](../features/event-streaming.md) |
| Diagnose runtime behavior | [Metrics and alerting](../features/prometheus-alerting.md), [provider health](../features/provider-health.md), and [distributed tracing](../features/distributed-tracing.md) |
| Retain and protect records | [Compliance mode](../features/compliance-mode.md), [payload encryption](../features/payload-encryption.md), and [retention](../features/data-retention.md) |
| Recover rejected stream inputs | [Quarantine inspection and audited repair](../features/stream-input-contracts.md) |
| Stop processing safely | [Audited stage halt/resume](../features/managed-stream-stages.md) and [agent lifecycle controls](../features/operator-lifecycle.md) |
| Resolve uncertain delivery | [Durable dispatch reconciliation](../features/durable-dispatch.md) and [managed outbox recovery](../features/managed-stream-outbox.md) |

Configure audit persistence and retention for the evidence you need. Execution
history, action audit, and stream control audits serve different purposes and
have different bounds. Halting input processing also has a different scope from
stopping output delivery or cancelling an agent run; use the control documented
for that component.

## Make governance part of deployment

Choose the workload's identity and scopes, define policy, select persistent state
and audit storage, and establish the recovery procedure. The
[deployment guide](../reference/deployment.md) and [backend guide](../backends/index.md)
connect these choices to a running system.
