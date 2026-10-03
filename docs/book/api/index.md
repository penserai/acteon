# Build and integrate

Connect applications, agents, workers, and operational tools to Acteon through
the interface that fits your environment. The HTTP API is the server contract;
SDKs, the CLI, MCP tools, and the Admin UI expose supported parts of that contract.

| Interface | Best starting point |
|---|---|
| [REST API](rest-api.md) | Dispatch actions and manage policy, executions, tasks, and operational state over HTTP |
| [Rust client](rust-client.md) | Integrate Rust services and use the opt-in managed stream HTTP source |
| [Python, TypeScript, Go, and Java SDKs](polyglot-clients.md) | Build application clients; Python and TypeScript also support code workflows |
| [MCP server](mcp-server.md) | Expose supported Acteon tools to an MCP-capable agent host |
| [CLI](cli.md) | Operate the platform from a terminal or automation script |
| [A2A](../features/a2a.md) | Discover agents and exchange interoperable tasks |
| [Agentic Bus](../concepts/agentic-bus.md) | Connect event producers, subscribers, and conversational agents |

## Start with the contract

An [action](../concepts/actions.md) names the namespace, tenant, provider, action
type, and payload. Its [outcome](../concepts/actions.md) describes the dispatch
result. Longer work has its own execution or run identity. Use the
[execution model](../concepts/execution-model.md) to choose the correct lifecycle
before wiring an integration.

The running server serves interactive Swagger UI at `/swagger-ui/` and its
OpenAPI document at `/api-doc/openapi.json`. Consult each SDK's feature coverage
and examples for the operations it supports; advanced capabilities vary by client.

## Configure access and policy

Use [authentication](authentication.md) and [scoped grants](../features/api-key-scoping.md)
to establish caller authority. Define policy with the [YAML rule reference](rule-reference.md)
and inspect it through [dry runs](../features/dry-run.md) or the
[rule playground](../features/rule-playground.md).

For complete compositions, see the [guides](../guides/index.md).
