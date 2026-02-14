# MCP Server

The Acteon MCP Server exposes the Acteon gateway to LLMs and AI agents via the [Model Context Protocol](https://modelcontextprotocol.io/). It enables agentic workflows for incident response, alert tuning, and automated operations.

## Installation

```bash
cargo build -p acteon-mcp-server
```

The binary is `acteon-mcp-server`. It communicates over **stdio** (stdin/stdout), which is the standard MCP transport for local integrations.

## Configuration

The server connects to a running Acteon gateway instance.

```bash
# Minimal — connects to localhost:8080
acteon-mcp-server

# Custom endpoint
acteon-mcp-server --endpoint http://acteon.internal:8080

# With API key authentication
acteon-mcp-server --api-key your-api-key
```

### Environment Variables

| Variable | Flag | Default | Description |
|----------|------|---------|-------------|
| `ACTEON_ENDPOINT` | `--endpoint` | `http://localhost:8080` | Gateway base URL |
| `ACTEON_API_KEY` | `--api-key` | _(none)_ | API key for authentication |

## Connecting to an MCP Host

### Claude Desktop

Add to your Claude Desktop configuration (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "acteon": {
      "command": "acteon-mcp-server",
      "args": ["--endpoint", "http://localhost:8080"],
      "env": {
        "ACTEON_API_KEY": "your-api-key"
      }
    }
  }
}
```

### Claude Code

Add to your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "acteon": {
      "command": "acteon-mcp-server",
      "args": ["--endpoint", "http://localhost:8080"]
    }
  }
}
```

### Generic MCP Host

Any MCP-compatible host can launch the server as a subprocess:

```bash
acteon-mcp-server --endpoint http://localhost:8080 --api-key your-key
```

The server reads JSON-RPC messages from stdin and writes responses to stdout. All logs go to stderr.

## Tools

The MCP server exposes the following tools to connected agents:

### `dispatch`

Send a new action through the Acteon gateway. Supports dry-run mode to preview rule evaluation without side effects.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | yes | Namespace for the action |
| `tenant` | string | yes | Tenant identifier |
| `provider` | string | yes | Target provider (e.g. `slack`, `email`) |
| `action_type` | string | yes | Action type discriminator |
| `payload` | object | yes | JSON payload for the provider |
| `metadata` | object | no | Key-value metadata labels |
| `dry_run` | boolean | no | Preview without executing |

### `query_audit`

Search the audit trail for historical dispatch records.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tenant` | string | no | Filter by tenant |
| `namespace` | string | no | Filter by namespace |
| `provider` | string | no | Filter by provider |
| `action_type` | string | no | Filter by action type |
| `outcome` | string | no | Filter by outcome (`executed`, `suppressed`, `failed`) |
| `limit` | integer | no | Max records (default 20) |

### `list_rules`

List all active routing and filtering rules loaded in the gateway. Returns rule name, priority, enabled status, and description.

### `evaluate_rules`

Run a test action through the rule engine without side effects. Returns a detailed per-rule evaluation trace showing which rules matched, were skipped, or errored.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | yes | Namespace |
| `tenant` | string | yes | Tenant |
| `provider` | string | yes | Provider |
| `action_type` | string | yes | Action type |
| `payload` | object | yes | Test payload |
| `include_disabled` | boolean | no | Include disabled rules in trace |

### `manage_event`

Transition a stateful event to a new state (acknowledge, resolve, investigate).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `fingerprint` | string | yes | Event fingerprint |
| `namespace` | string | yes | Namespace |
| `tenant` | string | yes | Tenant |
| `action` | string | yes | Target state (`acknowledged`, `resolved`, `investigating`) |

### `list_events`

List stateful events (open incidents, acknowledged alerts) for a namespace and tenant.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | yes | Namespace |
| `tenant` | string | yes | Tenant |
| `status` | string | no | Filter by state |
| `limit` | integer | no | Max events to return |

### `list_chains`

List action chains (multi-step workflows) for a tenant.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | yes | Namespace |
| `tenant` | string | yes | Tenant |
| `status` | string | no | Filter by status (`running`, `completed`) |

### `set_rule_enabled`

Enable or disable a routing rule by name.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `rule_name` | string | yes | Rule name |
| `enabled` | boolean | yes | `true` to enable, `false` to disable |

### `check_health`

Check if the Acteon gateway is healthy and responding. Takes no parameters.

## Resources

The server exposes read-only resources for retrieving current state:

| URI | Description |
|-----|-------------|
| `acteon://health` | Gateway health status |
| `acteon://rules` | All loaded routing rules |

### Resource Templates

| URI Template | Description |
|--------------|-------------|
| `acteon://audit/{tenant}` | Recent audit records for a tenant |
| `acteon://rules/{tenant}` | Active rule set for a tenant |
| `acteon://events/{tenant}` | Open stateful events for a tenant |

## Prompts

Pre-defined prompt templates guide the agent through common operational tasks:

### `investigate_incident`

Guides the agent to correlate events, check recent rule changes, and summarize the impact of an incident.

| Argument | Required | Description |
|----------|----------|-------------|
| `service` | yes | Service name to investigate |
| `tenant` | no | Tenant scope (default: `default`) |

### `optimize_alerts`

Analyzes notification volume and suggests grouping rules to reduce alert fatigue.

| Argument | Required | Description |
|----------|----------|-------------|
| `provider` | yes | Provider to analyze (e.g. `slack`) |
| `tenant` | no | Tenant scope (default: `default`) |

### `draft_guardrail`

Helps draft a natural language policy for LLM guardrails to gate sensitive notifications.

| Argument | Required | Description |
|----------|----------|-------------|
| `team` | yes | Team name to protect |
| `constraint` | no | Additional constraint to include |

## Agentic Workflow Examples

### Automated Root Cause Analysis

1. An external monitoring tool triggers a "High Latency" event in Acteon.
2. An MCP-connected agent receives a notification.
3. The agent calls `query_audit` to find correlated events in the same time window.
4. It discovers a `deploy_started` event and several `database_connection_error` events.
5. The agent calls `dispatch` to send a summary to Slack with its findings.

### Intelligent Alert Suppression

1. A database maintenance window begins.
2. An agent calls `set_rule_enabled` to activate a pre-configured suppression rule for DB alerts.
3. When maintenance finishes, the agent re-enables normal alerting and calls `manage_event` to resolve lingering alerts.

### Interactive Rule Debugging

1. An agent notices unexpected alert volume for a tenant.
2. It calls `evaluate_rules` with a sample payload to see which rules match.
3. The trace reveals a misconfigured priority causing the wrong rule to match first.
4. The agent reports its findings and suggests a fix.

## What's Next?

- [CLI](cli.md) -- command-line interface using the same operations layer
- [REST API](rest-api.md) -- direct HTTP access to the Acteon gateway
- [Rule Playground](../features/rule-playground.md) -- interactive rule evaluation in the admin UI
