# Agents and AI

Give an agent a way to act within the same operational boundaries as the rest of
your system. Acteon connects reasoning to execution through scoped interfaces,
policy, model contracts, approvals, and observable outcomes.

You can bring an existing agent, coordinate a team with Acteon's swarm
orchestrator, or use a small model inside an otherwise deterministic workflow.

## Choose the integration that fits

| Integration | Use it when… |
|---|---|
| [REST and SDKs](../api/index.md) | Your agent or application can submit actions and inspect outcomes directly |
| [MCP server](../api/mcp-server.md) | Your agent host needs Acteon's supported tools exposed through MCP |
| [A2A](a2a.md) | Agents need discoverable capabilities and interoperable task lifecycles |
| [Agentic Bus](../concepts/agentic-bus.md) | Long-running agents need identities, inboxes, conversations, typed tool messages, and streaming replies |
| [Agent Swarm](agent-swarm.md) | You want an orchestrator for coordinated roles, evaluation, adversarial critique, and recovery |
| [Ambient Swarm Provider](swarm-provider.md) | A configured Acteon action should accept a prepared swarm goal and return a run ID |

The swarm orchestrator is an optional runtime with its own engine dependencies.
The bus requires Kafka for its production transport. Build and configure those
components when your workload needs them; see [installation](../getting-started/installation.md).

## Use models as bounded components

The [governed model provider](governed-model-provider.md) calls a configured JSON
inference endpoint, verifies its declared model identity against a lock, injects
locked request material when configured, and validates the response schema.
That makes a classifier or scorer usable as an ordinary provider in rules and chains.

The model returns evidence or a decision value. Deterministic policy decides
what to do with it. Schema validation constrains the output contract; confidence
thresholds, corroboration, and approvals belong in the surrounding flow.

Other inference capabilities have different jobs:

- [LLM guardrails](llm-guardrails.md) evaluate content as part of action gating.
- [Semantic routing](semantic-routing.md) matches meaning to configured routes.
- Agent investigation can gather additional evidence before proposing a next action.

## Make authority explicit

Scope credentials to the agent's namespace, tenant, providers, and action types.
Set quotas and approvals for its operational actions. Use
[operator lifecycle controls](operator-lifecycle.md) for registered agents and
inspect the relevant task, run, execution, or action records.

Acteon governs operations routed through these integrations. Configure agent
tool hooks and external credentials to match that boundary. See the
[governance model](../concepts/governance.md) for how these controls fit together.

## Build a complete scenario

- [A2A and agent registry tutorial](../guides/a2a-agent-registry-tutorial.md): discover agents and follow task interactions.
- [Agent swarm coordination](../guides/agent-swarm-coordination.md): configure a coordinated team and its controls.
- [Cascading observability](../guides/neural-observability-detector.md): run real local Laya inference, deterministic incident routing, and bounded investigation over Kafka streams.
