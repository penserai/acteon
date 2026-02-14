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
