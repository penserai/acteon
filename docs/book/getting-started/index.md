# Start building with Acteon

Start with one action, see how policy changes its outcome, then add the execution
model your workload needs. The [quick start](quickstart.md) runs locally with
in-memory state and a log provider, so you can explore without external services.

## Your first working flow

1. [Install Acteon](installation.md) from source or build the container.
2. [Run the quick start](quickstart.md): dispatch an action, repeat it to observe
   deduplication, and send another that a rule suppresses.
3. [Choose an execution model](../concepts/execution-model.md) for your application.
4. [Configure the platform](configuration.md): providers, policy, identity, and storage.

For the source quick start, you need Rust 1.88+, Cargo, and curl. Docker is useful
for the container path, persistent backends, and the complete integration examples.

## Choose your path

| What are you building? | Start here | Then add |
|---|---|---|
| An application integration or notification service | [Actions](../concepts/actions.md) and [providers](../concepts/providers.md) | Rules, templates, quotas, and [durable admission](../features/durable-dispatch.md) |
| A business process that spans systems and waits | [Chains](../features/chains.md) | [Timers and signals](../features/durable-executions.md), approvals, and worker tasks |
| A workflow owned by your application code | [Python/TypeScript workflows](../features/workflows.md) | Worker queues, checkpoints, and child workflows |
| An agent application or coordinated agent team | [Agents and AI](../features/agents-and-ai.md) | Scoped tools, A2A or bus communication, and optional swarm orchestration |
| An event-driven detector or automation pipeline | [Streams and events](../features/streams-and-events.md) | Typed stages, model contracts, outbox delivery, and operational controls |

## From local exploration to a service

The platform includes the HTTP server, operational UI, CLI, MCP interface, and
client SDKs. Select the build features and services that support your deployment:
the default server build includes Redis support; Kafka bus and swarm support are
optional. Provider credentials, model endpoints, persistent stores, and custom
workers belong to your environment.

Use [configuration](configuration.md) for settings and
[deployment](../reference/deployment.md) for persistence, migrations, authentication,
and operations. The [guides](../guides/index.md) provide complete compositions
you can adapt to your domain.
