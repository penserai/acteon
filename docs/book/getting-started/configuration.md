# Configuration Reference

Acteon is configured via a TOML file (default: `acteon.toml`). Every section is optional — sensible defaults are provided for all values.

## CLI Options

```
cargo run -p acteon-server -- [OPTIONS]

Options:
  -c, --config <PATH>   Path to TOML config file [default: acteon.toml]
      --host <HOST>      Override bind host
      --port <PORT>      Override bind port
```

CLI flags override values in the config file.

## Full Configuration

```toml title="acteon.toml"
# ─── Server ───────────────────────────────────────────────
[server]
host = "127.0.0.1"                  # Bind address
port = 8080                          # Bind port
# shutdown_timeout_seconds = 30      # Graceful shutdown drain time

# ─── State Backend ────────────────────────────────────────
[state]
backend = "memory"                   # "memory" | "redis" | "postgres" | "dynamodb" | "clickhouse"
# url = "redis://localhost:6379"     # Connection URL (redis, postgres, clickhouse, dynamodb-local)
# prefix = "acteon"                  # Key/table prefix
# region = "us-east-1"              # AWS region (DynamoDB only)
# table_name = "acteon_state"       # Table name (DynamoDB only)

# ─── Audit Trail ──────────────────────────────────────────
[audit]
enabled = false                      # Enable audit recording
backend = "memory"                   # "memory" | "postgres" | "clickhouse" | "elasticsearch"
# url = "postgres://..."            # Connection URL
prefix = "acteon_"                   # Table/index prefix
ttl_seconds = 2592000                # Record TTL (30 days)
cleanup_interval_seconds = 3600      # Background cleanup interval
store_payload = true                 # Store action payloads in audit

# ─── Audit Redaction ──────────────────────────────────────
[audit.redact]
enabled = false                      # Enable field redaction
fields = ["password", "token", "api_key", "secret"]  # Fields to redact
placeholder = "[REDACTED]"           # Replacement text

# ─── Rules ────────────────────────────────────────────────
[rules]
# directory = "./rules"              # YAML rule files directory

# ─── Executor ─────────────────────────────────────────────
[executor]
max_retries = 3                      # Max retry attempts per action
timeout_seconds = 30                 # Per-action execution timeout
max_concurrent = 10                  # Max concurrent executions

# ─── Authentication ───────────────────────────────────────
[auth]
enabled = false                      # Enable authentication
# config_path = "auth.toml"         # Path to auth config
# watch = true                       # Hot-reload on file changes

# ─── Background Processing ───────────────────────────────
[background]
# tick_interval_ms = 1000           # Background loop tick interval
# group_flush_timeout_ms = 60000    # Group flush wait time
# timeout_check_batch_size = 100    # Batch size for timeout checks

# ─── State Machines ───────────────────────────────────────
[[state_machines]]
name = "alert"
initial_state = "firing"
states = ["firing", "acknowledged", "resolved", "stale"]

[[state_machines.transitions]]
from = "firing"
to = "acknowledged"

[[state_machines.transitions]]
from = "acknowledged"
to = "resolved"

[[state_machines.transitions]]
from = "firing"
to = "resolved"

[[state_machines.timeouts]]
state = "firing"
after_seconds = 3600
transition_to = "stale"

# ─── Task Chains ──────────────────────────────────────────
[[chains]]
name = "search-summarize-email"
on_failure = "abort"                 # "abort" | "abort_no_dlq"
# timeout_seconds = 604800          # Chain-level timeout (7 days)

[[chains.steps]]
name = "search"
provider = "search-api"
action_type = "web_search"
# delay_seconds = 0                 # Delay before execution
# on_failure = "abort"              # "abort" | "skip" | "dlq"

[[chains.steps]]
name = "summarize"
provider = "llm"
action_type = "summarize"

[[chains.steps]]
name = "send-email"
provider = "email"
action_type = "send_email"

# ─── LLM Guardrails ──────────────────────────────────────
[llm_guardrail]
# endpoint = "https://api.openai.com/v1/chat/completions"
# model = "gpt-4"
# api_key_env = "OPENAI_API_KEY"
# policy = "block"                  # "block" | "flag"
# temperature = 0.0
# max_tokens = 256

# ─── Embedding / Semantic Routing ────────────────────────
[embedding]
# enabled = false
# endpoint = "https://api.openai.com/v1/embeddings"
# model = "text-embedding-3-small"
# api_key = ""                      # Supports ENC[...] encrypted values
# timeout_seconds = 10
# fail_open = true                  # Return similarity 0.0 on API failure
# topic_cache_capacity = 10000      # Max cached topic embeddings
# topic_cache_ttl_seconds = 3600    # Topic cache TTL (1 hour)
# text_cache_capacity = 1000        # Max cached text embeddings
# text_cache_ttl_seconds = 60       # Text cache TTL (1 minute)
```

## Section Details

### `[server]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `"127.0.0.1"` | Bind address |
| `port` | u16 | `8080` | Bind port |
| `shutdown_timeout_seconds` | u64 | `30` | Max time to drain pending tasks on shutdown |

### `[state]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `backend` | string | `"memory"` | Backend type |
| `url` | string | — | Connection URL |
| `prefix` | string | `"acteon"` | Key prefix for all state entries |
| `region` | string | — | AWS region (DynamoDB only) |
| `table_name` | string | — | Table name (DynamoDB only) |

### `[audit]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable audit trail recording |
| `backend` | string | `"memory"` | Backend type |
| `url` | string | — | Connection URL |
| `prefix` | string | `"acteon_"` | Table/index prefix |
| `ttl_seconds` | u64 | `2592000` | Record time-to-live (30 days) |
| `cleanup_interval_seconds` | u64 | `3600` | Background cleanup frequency |
| `store_payload` | bool | `true` | Include action payloads in audit records |

### `[audit.redact]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable field redaction |
| `fields` | string[] | `["password", "token", "api_key", "secret"]` | Field names to redact |
| `placeholder` | string | `"[REDACTED]"` | Replacement text |

### `[rules]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `directory` | string | — | Path to directory containing YAML rule files |

!!! tip "Hot Reload"
    When a `directory` is specified, Acteon watches the directory for changes and automatically reloads rules. You can also trigger a manual reload via `POST /v1/rules/reload`.

### `[executor]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_retries` | u32 | `3` | Maximum retry attempts per action |
| `timeout_seconds` | u64 | `30` | Per-action execution timeout |
| `max_concurrent` | usize | `10` | Maximum concurrent action executions |

### `[auth]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable authentication |
| `config_path` | string | — | Path to auth configuration file |
| `watch` | bool | `true` | Hot-reload auth config on file changes |

See [Authentication](../api/authentication.md) for auth config file format.

### `[[state_machines]]`

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | State machine identifier (referenced in rules) |
| `initial_state` | string | State for new events |
| `states` | string[] | Valid state names |

#### `[[state_machines.transitions]]`

| Field | Type | Description |
|-------|------|-------------|
| `from` | string | Source state |
| `to` | string | Target state |

#### `[[state_machines.timeouts]]`

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | State that triggers timeout |
| `after_seconds` | u64 | Timeout duration |
| `transition_to` | string | Target state on timeout |

### `[[chains]]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | — | Chain identifier (referenced in rules) |
| `on_failure` | string | `"abort"` | `"abort"` or `"abort_no_dlq"` |
| `timeout_seconds` | u64 | 604800 | Overall chain timeout (7 days) |

See [Task Chains](../features/chains.md) for detailed chain configuration.

### `[embedding]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable the embedding provider |
| `endpoint` | string | `"https://api.openai.com/v1/embeddings"` | OpenAI-compatible embeddings API endpoint |
| `model` | string | `"text-embedding-3-small"` | Embedding model name |
| `api_key` | string | `""` | API key (supports `ENC[...]` encrypted values) |
| `timeout_seconds` | u64 | `10` | Request timeout |
| `fail_open` | bool | `true` | Return similarity `0.0` on API failure instead of erroring |
| `topic_cache_capacity` | u64 | `10000` | Max cached topic embeddings |
| `topic_cache_ttl_seconds` | u64 | `3600` | Topic cache TTL (1 hour) |
| `text_cache_capacity` | u64 | `1000` | Max cached text embeddings |
| `text_cache_ttl_seconds` | u64 | `60` | Text cache TTL (1 minute) |

!!! tip "Secret Management"
    The `api_key` field supports encrypted values. Set `ACTEON_AUTH_KEY` and use `acteon-server encrypt` to generate an `ENC[...]` token. See [Semantic Routing](../features/semantic-routing.md) for details.

See [Semantic Routing](../features/semantic-routing.md) for feature documentation.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log verbosity (`error`, `warn`, `info`, `debug`, `trace`) |
| `OPENAI_API_KEY` | API key for LLM guardrail evaluations |
| `ACTEON_AUTH_KEY` | Hex-encoded 256-bit master key for decrypting `ENC[...]` config values |

## Example Configurations

Ready-to-use configs are in the `examples/` directory:

| File | Description |
|------|-------------|
| `examples/redis.toml` | Redis state backend |
| `examples/postgres.toml` | PostgreSQL state + audit |
| `examples/clickhouse.toml` | ClickHouse state + audit |
| `examples/elasticsearch-audit.toml` | Redis state + Elasticsearch audit |
| `examples/dynamodb.toml` | DynamoDB state backend |
| `examples/full.toml` | All options documented |

```bash
# Start with Redis
docker compose up -d
cargo run -p acteon-server -- -c examples/redis.toml

# Start with PostgreSQL
docker compose --profile postgres up -d
cargo run -p acteon-server -- -c examples/postgres.toml
```
