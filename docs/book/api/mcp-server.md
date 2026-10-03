# MCP Server

The Acteon MCP Server exposes supported Acteon execution and operational tools to LLMs and AI agents via the [Model Context Protocol](https://modelcontextprotocol.io/). It enables agentic workflows for incident response, alert tuning, and automated operations.

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

The MCP server exposes 46 specialized tools to connected agents, organized across seven operational domains:

### Core Action Dispatch & Rules

| Tool | Parameters | Description |
|------|------------|-------------|
| `dispatch` | `namespace`, `tenant`, `provider`, `action_type`, `payload`, `metadata?`, `dry_run?` | Dispatch action through gateway with optional dry-run preview |
| `list_rules` | _(none)_ | List loaded routing and filtering rules |
| `evaluate_rules` | `namespace`, `tenant`, `provider`, `action_type`, `payload`, `include_disabled?` | Dry-run trace showing matched, skipped, and errored rules |
| `set_rule_enabled` | `rule_name`, `enabled` | Dynamically enable or disable a rule |
| `check_health` | _(none)_ | Gateway health check and provider status |

### Audit Trail & Analytics

| Tool | Parameters | Description |
|------|------------|-------------|
| `query_audit` | `tenant?`, `namespace?`, `provider?`, `action_type?`, `outcome?`, `limit?` | Filter historical dispatch records |
| `replay_audit` | `tenant`, `namespace`, `action_id` | Reconstruct and re-dispatch action from audit trail |
| `analytics_volume` | `tenant`, `namespace`, `window_hours?` | Query total action throughput volume |
| `analytics_latency` | `tenant`, `namespace`, `window_hours?` | Query execution latency percentiles (p50, p90, p99) |
| `analytics_error_rate` | `tenant`, `namespace`, `window_hours?` | Query failure and error rates |
| `analytics_outcomes` | `tenant`, `namespace`, `window_hours?` | Query breakdown by outcome (`executed`, `suppressed`, `deduplicated`) |
| `analytics_top_actions` | `tenant`, `namespace`, `limit?` | Query top action types by frequency |

### Event Lifecycle & Grouping

| Tool | Parameters | Description |
|------|------------|-------------|
| `list_events` | `namespace`, `tenant`, `status?`, `limit?` | List stateful events (open, acknowledged, resolved) |
| `manage_event` | `fingerprint`, `namespace`, `tenant`, `action` | Transition event to new state (`acknowledged`, `resolved`, `investigating`) |
| `list_groups` | `namespace`, `tenant` | List active consolidated notification batches |
| `flush_group` | `group_key` | Force flush/close an active notification group |

### Task Chains & Definitions

| Tool | Parameters | Description |
|------|------------|-------------|
| `list_chains` | `namespace`, `tenant`, `status?` | List multi-step workflow chain runs |
| `get_chain` | `chain_id` | Retrieve chain state and step progress |
| `cancel_chain` | `chain_id` | Cancel an in-flight task chain |
| `get_chain_dag` | `chain_id` | Inspect chain execution Directed Acyclic Graph (DAG) |
| `list_chain_definitions` | `namespace`, `tenant` | List reusable chain definition templates |

### Agentic Message Bus

| Tool | Parameters | Description |
|------|------------|-------------|
| `bus_list_topics` | _(none)_ | List Kafka-backed message bus topics |
| `bus_create_topic` | `name`, `partitions?`, `replication_factor?` | Create new bus topic |
| `bus_publish` | `topic`, `payload`, `key?`, `headers?` | Publish message with schema validation |
| `bus_subscribe_url` | `subscription_id` | Get SSE subscription URL for stream consumption |
| `bus_list_subscriptions` | `namespace`, `tenant` | List durable consumer subscriptions |
| `bus_ack_subscription` | `namespace`, `tenant`, `id`, `offset` | Acknowledge processed messages up to offset |
| `bus_subscription_lag` | `namespace`, `tenant`, `id` | Query subscription consumer lag |
| `bus_deadletter_subscription`| `namespace`, `tenant`, `id`, `reason` | Dead-letter unprocessable message |
| `bus_list_schemas` | `namespace`, `tenant` | List registered JSON schemas |
| `bus_create_schema` | `namespace`, `tenant`, `subject`, `schema` | Register new schema version |
| `bus_bind_schema` | `namespace`, `tenant`, `topic`, `subject` | Bind topic to schema subject for publish validation |
| `bus_list_agents` | `namespace`, `tenant` | List registered autonomous agents and liveness |
| `bus_register_agent` | `namespace`, `tenant`, `agent_id`, `name`, `role` | Register agent in bus registry |
| `bus_heartbeat_agent` | `namespace`, `tenant`, `agent_id` | Record agent liveness heartbeat |
| `bus_send_to_agent` | `namespace`, `tenant`, `agent_id`, `message` | Route message to an agent's inbox |
| `bus_list_conversations` | `namespace`, `tenant` | List multi-agent conversation threads |
| `bus_conversation_messages`| `namespace`, `tenant`, `conversation_id` | Replay conversation message history |
| `bus_post_tool_call` | `namespace`, `tenant`, `conversation_id`, `tool_name`, `args` | Post tool-call envelope to thread |
| `bus_lookup_tool_result` | `namespace`, `tenant`, `call_id` | Retrieve execution result for tool call |
| `bus_post_stream_chunk` | `namespace`, `tenant`, `conversation_id`, `stream_id`, `seq`, `content` | Stream token or audio chunk |
| `bus_post_stream_end` | `namespace`, `tenant`, `conversation_id`, `stream_id` | Finalize streaming response |
| `bus_list_approvals` | `namespace`, `tenant` | List pending HITL tool-call approvals |
| `bus_approve` | `namespace`, `tenant`, `approval_id` | Approve pending tool execution |
| `bus_reject` | `namespace`, `tenant`, `approval_id`, `reason?` | Reject pending tool execution |

### Operations, Governance & Resilience

| Tool | Parameters | Description |
|------|------------|-------------|
| `list_recurring` | `namespace`, `tenant` | List cron-scheduled recurring actions |
| `pause_recurring` | `id` | Pause recurring action schedule |
| `resume_recurring` | `id` | Resume recurring action schedule |
| `list_quotas` | `namespace`, `tenant` | List tenant quota policies |
| `get_quota_usage` | `quota_id` | Inspect real-time quota usage and remaining units |
| `list_silences` | `namespace`, `tenant` | List active alert silences |
| `create_silence` | `namespace`, `tenant`, `matchers`, `starts_at`, `ends_at`, `comment` | Create time-bounded alert silence |
| `delete_silence` | `id` | Expire or remove an alert silence |
| `dlq_stats` | _(none)_ | Dead-letter queue statistics and unhandled error counts |
| `dlq_drain` | `limit?` | Drain DLQ entries for reprocessing |
| `list_circuit_breakers` | _(none)_ | Check circuit breaker status across all providers |
| `trip_circuit_breaker` | `provider` | Administratively trip provider circuit breaker |
| `reset_circuit_breaker` | `provider` | Reset provider circuit breaker to Closed |
| `compliance_status` | _(none)_ | Check active compliance mode (SOC2/HIPAA) |
| `verify_compliance` | `namespace`, `tenant` | Cryptographically verify SHA-256 audit log hash chain |

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
