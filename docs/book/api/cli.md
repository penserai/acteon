# CLI

The `acteon-cli` binary provides a command-line interface for interacting with the Acteon gateway. It shares the same operations layer as the [MCP Server](mcp-server.md), so every capability exposed to AI agents is also available from the terminal.

## Installation

```bash
cargo build -p acteon-cli
```

## Configuration

```bash
# Minimal — connects to localhost:8080
acteon-cli health

# Custom endpoint
acteon-cli --endpoint http://acteon.internal:8080 health

# With API key
acteon-cli --api-key your-key health
```

### Environment Variables

| Variable | Flag | Default | Description |
|----------|------|---------|-------------|
| `ACTEON_ENDPOINT` | `--endpoint` | `http://localhost:8080` | Gateway base URL |
| `ACTEON_API_KEY` | `--api-key` | _(none)_ | API key for authentication |

### Output Format

All commands support `--format text` (default) or `--format json` for machine-readable output:

```bash
acteon-cli --format json rules list
```

## Commands

### `health`

Check gateway health:

```bash
acteon-cli health
```

### `dispatch`

Send an action through the gateway:

```bash
# Inline JSON payload
acteon-cli dispatch \
  --tenant prod \
  --provider slack \
  --type send_alert \
  --payload '{"channel": "#ops", "message": "Deploy complete"}'

# Payload from file
acteon-cli dispatch \
  --tenant prod \
  --provider email \
  --type send_email \
  --payload @payload.json

# With metadata labels
acteon-cli dispatch \
  --tenant prod \
  --provider webhook \
  --type notify \
  --payload '{"url": "https://example.com"}' \
  --metadata severity=critical \
  --metadata team=infra

# Dry-run mode (preview without executing)
acteon-cli dispatch \
  --tenant prod \
  --provider slack \
  --type send_alert \
  --payload '{"message": "test"}' \
  --dry-run
```

| Flag | Required | Description |
|------|----------|-------------|
| `--namespace` | no | Namespace (default: `default`) |
| `--tenant` | yes | Tenant identifier |
| `--provider` | yes | Target provider |
| `--type` | yes | Action type |
| `--payload` | yes | JSON string or `@file` path |
| `--metadata` | no | Key=value pairs (repeatable) |
| `--dry-run` | no | Preview without executing |

### `audit`

Query the audit trail:

```bash
# Recent records for a tenant
acteon-cli audit --tenant prod

# Filter by outcome
acteon-cli audit --tenant prod --outcome suppressed

# Filter by provider and limit
acteon-cli audit --tenant prod --provider slack --limit 50
```

| Flag | Required | Description |
|------|----------|-------------|
| `--tenant` | no | Filter by tenant |
| `--namespace` | no | Filter by namespace |
| `--provider` | no | Filter by provider |
| `--action-type` | no | Filter by action type |
| `--outcome` | no | Filter by outcome |
| `--limit` | no | Max records (default 20) |

### `rules`

Manage routing rules:

```bash
# List all rules
acteon-cli rules list

# Enable a rule
acteon-cli rules enable block-spam

# Disable a rule
acteon-cli rules disable noisy-alerts
```

### `events`

Manage stateful events:

```bash
# List events for a tenant
acteon-cli events list --namespace alerts --tenant prod

# Filter by state
acteon-cli events list --namespace alerts --tenant prod --status open

# Acknowledge an event
acteon-cli events manage \
  --fingerprint abc123 \
  --namespace alerts \
  --tenant prod \
  --action acknowledged

# Resolve an event
acteon-cli events manage \
  --fingerprint abc123 \
  --namespace alerts \
  --tenant prod \
  --action resolved
```

### `chains`

Manage task chain executions, definitions, and DAGs:

```bash
# List active and completed chains
acteon chains list --namespace alerts --tenant prod

# Inspect a specific chain
acteon chains get <chain-id>

# Cancel a running chain
acteon chains cancel <chain-id>

# Inspect chain DAG structure
acteon chains dag <chain-id>

# View execution step history
acteon chains history <chain-id>

# List reusable chain definitions
acteon chains definitions list --namespace alerts --tenant prod
```

### `bus`

Drive the agentic message bus (topics, subscriptions, schemas, agents, conversations, tools, streams, approvals):

```bash
# Topics
acteon bus topics list
acteon bus topics create <topic-name>

# Publish message
acteon bus publish --topic alerts.critical --payload '{"event":"cpu_spike"}'

# Subscriptions
acteon bus subscriptions list --namespace alerts --tenant prod
acteon bus subscriptions lag --namespace alerts --tenant prod --id <sub-id>
acteon bus subscriptions ack --namespace alerts --tenant prod --id <sub-id> --offset 1234

# Agents
acteon bus agents list --namespace agents --tenant prod
acteon bus agents heartbeat --namespace agents --tenant prod --agent-id <agent-id>
acteon bus agents send --namespace agents --tenant prod --agent-id <agent-id> --message '{"cmd":"run"}'

# Conversations
acteon bus conversations list --namespace agents --tenant prod
acteon bus conversations messages --namespace agents --tenant prod --conversation-id <id>

# Tool Calls & HITL Approvals
acteon bus tools post-call --namespace agents --tenant prod --conversation-id <id> --tool-name "restart_svc" --args '{"svc":"auth"}'
acteon bus approvals list --namespace agents --tenant prod
acteon bus approvals approve --namespace agents --tenant prod --id <approval-id>
```

### `recurring`

Manage cron-scheduled recurring actions:

```bash
# List recurring actions
acteon recurring list --namespace periodic --tenant prod

# Pause and resume recurring action
acteon recurring pause --id <id>
acteon recurring resume --id <id>

# Trigger immediate backfill run
acteon recurring backfill --id <id>
```

### `quotas`

Manage tenant quota limits and usage tracking:

```bash
# List quota policies
acteon quotas list --namespace alerts --tenant prod

# Inspect current quota usage and remaining allowance
acteon quotas usage --id <quota-id>

# Reload static quota policies from file
acteon quotas reload
```

### `silences`

Manage Alertmanager-compatible alert silences:

```bash
# List silences
acteon silences list --namespace alerts --tenant prod

# Create a silence
acteon silences create \
  --namespace alerts \
  --tenant prod \
  --matcher "alertname=HighCPU" \
  --matcher "cluster=prod-us" \
  --starts-at "2026-03-01T00:00:00Z" \
  --ends-at "2026-03-01T04:00:00Z" \
  --comment "Maintenance window" \
  --created-by "ops-team"

# Delete / expire a silence
acteon silences delete <silence-id>
```

### `retention`

Manage data retention policies for audit and DLQ records:

```bash
# List retention policies
acteon retention list --namespace alerts --tenant prod

# Inspect specific policy
acteon retention get <policy-id>
```

### `templates`

Manage MiniJinja payload templates and mapping profiles:

```bash
# List templates
acteon templates list --namespace alerts --tenant prod

# Test render a template preview
acteon templates render \
  --content "Alert: {{ summary }} on {{ cluster }}" \
  --context '{"summary":"CPU spike","cluster":"prod-1"}'

# Reload static template manifests
acteon templates reload

# List profiles
acteon templates profiles list --namespace alerts --tenant prod
```

### `plugins`

Inspect and manage WebAssembly (WASM) rule plugins:

```bash
# List registered plugins and memory limits
acteon plugins list

# Unregister a dynamic plugin
acteon plugins unregister <plugin-name>
```

### `groups`

Inspect and manage event grouping batches:

```bash
# List active groups
acteon groups list --namespace alerts --tenant prod

# Force flush / close an active group
acteon groups flush <group-key>
```

### `dlq`

Inspect and drain the dead-letter queue:

```bash
# View DLQ statistics and failure counts
acteon dlq stats

# Drain DLQ records for reprocessing
acteon dlq drain --limit 100
```

### `compliance`

Inspect compliance status and verify audit chain integrity:

```bash
# Inspect active compliance mode (SOC2 / HIPAA)
acteon compliance status

# Cryptographically verify the SHA-256 audit hash chain
acteon compliance verify --namespace alerts --tenant prod
```

### `providers`

Check real-time provider health, circuit breaker state, and latencies:

```bash
# List provider health status
acteon providers health
```

### `approvals`

Review and decide on Human-In-The-Loop (HITL) approval gates:

```bash
# List pending approvals
acteon approvals list --namespace alerts --tenant prod

# Approve or reject an action
acteon approvals approve --id <approval-id>
acteon approvals reject --id <approval-id> --reason "Deploy window closed"
```

### `import`

Import alert configurations from external systems:

```bash
# Import an Alertmanager YAML configuration file
acteon import alertmanager -f alertmanager.yml --output-dir ./rules
```

### `keys`

Manage local Ed25519 action signing keys:

```bash
# Generate a new Ed25519 signing keypair
acteon keys generate --name "prod-key-2026" --output-dir ./keys

# List keys in keyring
acteon keys list --key-dir ./keys

# Rotate active key
acteon keys rotate --key-dir ./keys --old-key <id> --new-key <id>
```

### `metrics`

Export Prometheus alerting rules generated from the running server
configuration:

```bash
acteon-cli metrics export-alerts --output acteon-alerts.yml
```

Without `--output`, the YAML is written to stdout. The server-side source is
also available at `GET /v1/metrics/alerts/prometheus.yaml`.

## JSON Output

Use `--format json` for scripting and piping:

```bash
# Get rules as JSON and pipe to jq
acteon-cli --format json rules list | jq '.[].name'

# Dispatch and capture outcome
OUTCOME=$(acteon-cli --format json dispatch \
  --tenant prod --provider slack --type alert \
  --payload '{"msg": "test"}')
echo "$OUTCOME" | jq '.status'
```

## What's Next?

- [MCP Server](mcp-server.md) -- expose the same capabilities to AI agents
- [REST API](rest-api.md) -- direct HTTP access
- [Rust Client](rust-client.md) -- programmatic access from Rust
