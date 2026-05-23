# MCP Server & CLI Architecture

## Overview

The MCP server and CLI share a **Common Operations Layer** (`acteon-ops` crate)
that wraps the Acteon HTTP client with configuration management and high-level
convenience methods. Both binaries are thin presentation layers on top of this
shared logic.

```
┌─────────────────┐   ┌──────────────────┐
│  MCP Host / LLM │   │  Terminal / User  │
└────────┬────────┘   └────────┬─────────┘
         │ stdio (JSON-RPC)    │ CLI args
         ▼                     ▼
┌─────────────────┐   ┌──────────────────┐
│ acteon-mcp-server│   │   acteon-cli     │
│   (rmcp SDK)    │   │   (clap)         │
└────────┬────────┘   └────────┬─────────┘
         │                     │
         ▼                     ▼
     ┌──────────────────────────────┐
     │     acteon-ops (OpsClient)   │
     │  DispatchOptions, OpsConfig  │
     └──────────────┬───────────────┘
                    │
                    ▼
     ┌──────────────────────────────┐
     │   acteon-client (HTTP)       │
     │  ActeonClient, ActeonBuilder │
     └──────────────┬───────────────┘
                    │ HTTP
                    ▼
     ┌──────────────────────────────┐
     │      Acteon Gateway          │
     └──────────────────────────────┘
```

## Crate Layout

```
crates/
├── ops/                  # Common operations layer
│   ├── src/
│   │   ├── lib.rs        # OpsClient, DispatchOptions, re-exports
│   │   ├── config.rs     # OpsConfig (endpoint, api_key, timeout)
│   │   └── error.rs      # OpsError (Configuration, Client)
│   └── Cargo.toml
│
├── mcp-server/           # MCP server binary
│   ├── src/
│   │   ├── main.rs       # Entry point, stdio transport
│   │   ├── server.rs     # ActeonMcpServer, ServerHandler impl
│   │   ├── tools.rs      # 10 MCP tools with parameter types
│   │   ├── resources.rs  # Static + template resources
│   │   └── prompts.rs    # 3 operational prompt templates
│   └── Cargo.toml
│
├── cli/                  # CLI binary
│   ├── src/
│   │   ├── main.rs       # Entry point, clap command routing
│   │   └── commands/     # One module per subcommand
│   │       ├── audit.rs
│   │       ├── dispatch.rs
│   │       ├── events.rs
│   │       ├── health.rs
│   │       └── rules.rs
│   └── Cargo.toml
```

## Operations Layer (`acteon-ops`)

`OpsClient` wraps `ActeonClient` in an `Arc` and provides high-level methods
that both consumers call:

| Method | Description |
|--------|-------------|
| `dispatch()` | Build `Action` from params, apply metadata, route to normal/dry-run |
| `query_audit()` | Forward `AuditQuery` to client |
| `list_rules()` | Retrieve all loaded rules |
| `evaluate_rules()` | Build `Action` + `EvaluateRulesOptions`, invoke trace endpoint |
| `transition_event()` | Forward state transition |
| `list_events()` | Forward `EventQuery` |
| `list_chains()` | Forward chain listing |
| `set_rule_enabled()` | Toggle rule enabled state |
| `health()` | Gateway health check |

Configuration is loaded from environment variables (`ACTEON_ENDPOINT`,
`ACTEON_API_KEY`, `ACTEON_TIMEOUT_SECS`) or CLI flags, then passed as
`OpsConfig` to `OpsClient::from_config()`.

Re-exports: `acteon_ops::acteon_client` and `acteon_ops::acteon_core` provide
downstream crates access to client and core types without direct dependencies.

## MCP Server (`acteon-mcp-server`)

Built on the [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk) SDK
(v0.15+). The server implements the `ServerHandler` trait with three capability
categories:

### Tools (10 tools)

Each tool is a method on `ActeonMcpServer` annotated with `#[tool]` inside a
`#[tool_router]` impl block. Parameter types derive `schemars::JsonSchema` for
automatic schema generation. The macro generates a private `tool_router()` fn;
a `pub(crate) create_tool_router()` wrapper exposes it to `server.rs`.

Tools delegate to `self.ops.<method>()` and return `CallToolResult` with
JSON-serialized responses. Errors are returned as `CallToolResult::error()`
rather than MCP protocol errors, so the agent always gets a useful message.

### Resources

Two static resources (`acteon://health`, `acteon://rules`) and three URI
templates (`acteon://audit/{tenant}`, `acteon://rules/{tenant}`,
`acteon://events/{tenant}`). Resource reads call the ops layer and return
JSON content.

### Prompts (3 prompts)

Guided operational workflows with typed argument structs:

- `investigate_incident(service, tenant?)` -- incident correlation
- `optimize_alerts(provider, tenant?)` -- alert fatigue analysis
- `draft_guardrail(team, constraint?)` -- LLM policy drafting

Each returns a `GetPromptResult` with multi-message instructions for the agent.

### Transport

Stdio only (Phase 1). The server reads JSON-RPC from stdin and writes to
stdout. All logging uses `tracing` directed to stderr so it doesn't interfere
with the MCP protocol.

## CLI (`acteon-cli`)

Built on `clap` v4 with derive mode. Subcommands: `health`, `dispatch`,
`audit`, `rules` (list/enable/disable), `events` (list/manage).

Each command module receives `&OpsClient` and `&OutputFormat`, calls ops-layer
methods, and prints results as either `Debug` text or pretty-printed JSON.

The `dispatch` command supports `@file` payload syntax and repeatable
`--metadata key=value` flags.

## Design Decisions

1. **Shared ops layer**: Both binaries share identical logic. A bug fix in
   `acteon-ops` automatically benefits both.

2. **Errors as tool results**: MCP tools return errors via `CallToolResult::error()`
   instead of MCP-level error codes, giving agents descriptive messages they
   can reason about.

3. **Schema generation**: `schemars` v1 (re-exported by `rmcp`) auto-generates
   JSON Schema for every tool parameter struct. No manual schema maintenance.

4. **Macro visibility workaround**: `#[tool_router]` generates a private method.
   The `pub(crate) create_tool_router()` wrapper pattern keeps the tool
   definitions co-located with their implementations while allowing `server.rs`
   to construct the router.

5. **Re-export modules**: `acteon_ops::acteon_client` and `acteon_ops::acteon_core`
   re-export all types so downstream crates (MCP server, CLI) don't need direct
   dependencies on `acteon-client` or `acteon-core`.
