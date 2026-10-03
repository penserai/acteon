# Choose your execution model

Start with the smallest unit that owns the work you need. A notification may
need one governed action. An order may need a chain with a human decision and a
durable timer. A telemetry detector may need a stream stage that produces an
action for a separate execution.

These building blocks compose: a stream can emit a verdict, a rule can select a
chain, a chain can call a model or a worker, and a human can authorize the next step.

## Match the work to the primitive

| You need to… | Use | It owns |
|---|---|---|
| Call an integration under policy | [Action dispatch](actions.md) | Rule evaluation, provider execution, and an explicit outcome |
| Accept retried deliveries with a durable receipt | [Durable dispatch](../features/durable-dispatch.md) | Persisted acceptance, request identity, execution linkage, and outcome recovery |
| Orchestrate a known sequence with branching or parallel work | [Chains](../features/chains.md) | Steps, data flow, failure policies, and a tracked execution |
| Pause work across restarts | [Durable executions](../features/durable-executions.md) | Timers, signals, pinned chain definitions, and event history |
| Run custom code in your environment | [Task queues and workers](../features/task-queues.md) | Task claims, leases, heartbeats, retry, and completion |
| Express orchestration in Python or TypeScript | [Workflows as code](../features/workflows.md) | Checkpoints, continuations, timers, signals, and child workflows |
| Address agents and exchange contextual messages | [Agentic Bus](agentic-bus.md) or [A2A](../features/a2a.md) | Messaging and conversations, or interoperable agent tasks |
| Process ordered event streams and recover progress | [Managed stream stages](../features/managed-stream-stages.md) | Bounded batches, typed inputs, callback state, source progress, and output checkpoints |
| Run a coordinated agent plan | [Agent Swarm](../features/agent-swarm.md) | Agent roles, execution cycles, evaluation, critique, and recovery |

## A concrete composition

Consider a database incident:

1. A stream stage correlates metrics, traces, and logs into an event-time window.
2. Governed model calls return schema-validated signal classifications.
3. Deterministic policy decides whether to suppress noise, start an incident
   chain, or request further investigation.
4. The chain gathers diagnostics and routes operational actions through providers.
5. A configured approval gate can require a human decision for sensitive work.
6. Operators inspect evidence and recover failures through the relevant stage,
   execution, or delivery controls.

The [runnable observability guide](../guides/neural-observability-detector.md)
demonstrates stream recovery, typed inference, deterministic routing, an incident
chain, and a bounded investigator. It records operational effects locally so the
scenario can be run without paging a real on-call team.

## Understand the completion boundary

**Acceptance, execution, and external effect are different milestones.** An
`ActionOutcome` describes the dispatch result. `ChainStarted` links to a durable
execution whose steps may still be running. A swarm provider returns a run ID
before its background work completes. A completed model call returns validated
data; downstream policy still decides what action to take.

Durability comes from the chosen persistent backend and each primitive's recovery
protocol. Completed checkpoints can prevent repeated work, while a crash before
a checkpoint may cause a callback or workflow step to run again. External effects
need provider idempotency or explicit reconciliation where their outcome is
uncertain. See [durable dispatch](../features/durable-dispatch.md),
[workflow semantics](../features/workflows.md), and
[stream checkpoints](../features/stream-checkpoints.md) for the exact boundaries.

## Put the right code in the right place

Keep policy in rules and grants. Keep integration code in providers. Put custom
business logic in workers and typed stream processors. Use chains for declarative
orchestration and code workflows when control flow belongs in code. Introduce
model or agent reasoning where it helps decide what should happen, then route
the resulting operations through the configured execution controls.
