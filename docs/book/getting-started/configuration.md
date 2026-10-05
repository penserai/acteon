# Configuration Reference

Acteon is configured via a TOML file (default: `acteon.toml`). Defaults support a small local setup; enable and configure the providers,
identity controls, storage, and optional runtimes your deployment uses. The
[platform overview](../concepts/index.md) explains how these components fit together.

## CLI Options

```
cargo run -p acteon-server -- [OPTIONS] [COMMAND]

Options:
  -c, --config <PATH>   Path to TOML config file [default: acteon.toml]
      --host <HOST>      Override bind host
      --port <PORT>      Override bind port

Commands:
  encrypt   Encrypt a value for use in auth.toml (reads from stdin)
  migrate   Run database migrations for configured backends, then exit
```

CLI flags override values in the config file.

### Database Migrations

Before starting the server for the first time (or after upgrading), run migrations to create or update database schemas:

```bash
# Using the wrapper script (auto-detects backend from config)
scripts/migrate.sh -c acteon.toml

# Or directly via cargo
cargo run -p acteon-server --features postgres -- -c acteon.toml migrate
```

Migrations are idempotent and safe to run multiple times. They use `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE ADD COLUMN IF NOT EXISTS` patterns. Backends that don't require schemas (memory, Redis) are no-ops.

## Full Configuration

```toml title="acteon.toml"
# ─── Server ───────────────────────────────────────────────
[server]
host = "127.0.0.1"                  # Bind address
port = 8080                          # Bind port
# shutdown_timeout_seconds = 30      # Graceful shutdown drain time
# max_sse_connections_per_tenant = 10  # Max concurrent SSE connections per tenant

# ─── State Backend ────────────────────────────────────────
[state]
backend = "memory"                   # "memory" | "redis" | "postgres" | "dynamodb"
# url = "redis://localhost:6379"     # Connection URL (redis, postgres, dynamodb-local)
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
dlq_enabled = false                  # Enable the dead-letter queue
# dlq_retention_seconds = 604800      # Optional DLQ retention (7 days)

# ─── Providers ───────────────────────────────────────────
# [[providers]]
# name = "email"
# type = "email"
# from_address = "noreply@example.com"

# [[providers]]
# name = "alert-fanout"
# type = "aws-sns"
# aws_region = "us-east-1"
# topic_arn = "arn:aws:sns:us-east-1:123:alerts"
# aws_endpoint_url = "http://localhost:4566"  # LocalStack

# [[providers]]
# name = "archive"
# type = "aws-s3"
# aws_region = "us-east-1"
# bucket_name = "my-bucket"
# object_prefix = "acteon/"

# ─── Authentication ───────────────────────────────────────
# Users and API keys live in auth.toml, a separate file referenced from
# here. Each principal is authorized via a list of grants that scope them
# to specific tenants, namespaces, providers, and action types. Tenant
# grants support hierarchical matching — a grant on "acme" covers
# "acme.us-east" and "acme.us-east.prod". See the "API Key Scoping"
# feature page for the full grant model and worked examples.
[auth]
enabled = false                      # Enable authentication
# config_path = "auth.toml"         # Path to auth config (relative to acteon.toml)
# watch = true                       # Hot-reload auth.toml on file changes

# ─── Background Processing ───────────────────────────────
[background]
# tick_interval_ms = 1000           # Background loop tick interval
# group_flush_timeout_ms = 60000    # Group flush wait time
# timeout_check_batch_size = 100    # Batch size for timeout checks
# enable_scheduled_actions = false  # Enable scheduled action processing
# scheduled_check_interval = 5     # Scheduled action poll interval (seconds)
# enable_recurring_actions = false  # Enable recurring action processing
# recurring_check_interval_seconds = 5  # Recurring action poll interval (seconds)

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

# ─── Circuit Breaker ─────────────────────────────────────
[circuit_breaker]
enabled = false                      # Enable circuit breakers
failure_threshold = 5                # Consecutive failures to open
success_threshold = 2                # Consecutive successes to close
recovery_timeout_seconds = 60        # Seconds before probing

# Per-provider overrides
# [circuit_breaker.providers.email]
# failure_threshold = 10
# recovery_timeout_seconds = 120
# fallback_provider = "webhook"

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

# ─── Telemetry / Distributed Tracing ────────────────────
[telemetry]
# enabled = false                    # Enable OpenTelemetry tracing
# endpoint = "http://localhost:4317" # OTLP collector endpoint
# service_name = "acteon"            # Service name in traces
# sample_ratio = 1.0                 # 0.0 (none) to 1.0 (all)
# protocol = "grpc"                  # "grpc" (port 4317) or "http" (port 4318)
# timeout_seconds = 10               # Exporter timeout

# [telemetry.resource_attributes]
# "deployment.environment" = "production"
# "service.instance.id" = "acteon-01"

# ─── Payload Encryption at Rest ──────────────────────────
[encryption]
# enabled = false                    # Enable AES-256-GCM payload encryption

# ─── Tenant Quota Enforcement ────────────────────────────
[quotas]
# enabled = true                     # Enable tenant quota limits
# default_window = "daily"          # Default quota window
# default_overage_behavior = "block" # "block" | "drop" | "warn"
# policies_file = "quotas.toml"     # Static quota policies manifest
# watch = true                       # Hot-reload quotas file on changes

# ─── Payload Templates & Profiles ────────────────────────
[templates]
# manifest_file = "templates.toml"  # TOML template/profile manifest
# watch = true                       # Hot-reload templates on changes

# ─── Compliance Mode ─────────────────────────────────────
[compliance]
# mode = "soc2"                      # "none" | "soc2" | "hipaa"
# sync_audit_writes = true           # Force synchronous audit writes
# immutable_audit = false            # Prevent audit record mutation
# hash_chain = true                  # Cryptographic SHA-256 chain verification

# ─── WASM Plugin Runtime ─────────────────────────────────
[wasm]
# enabled = false                    # Enable WASM plugin execution
# plugin_dir = "./plugins"           # Directory containing .wasm files
# default_memory_limit_bytes = 16777216  # 16 MB limit per plugin
# default_timeout_ms = 100           # Per-execution timeout

# ─── TLS / Mutual TLS (mTLS) ─────────────────────────────
[tls]
# enabled = false
# cert_path = "certs/server.crt"
# key_path = "certs/server.key"
# client_ca_path = "certs/ca.crt"   # Enable mTLS client verification
# require_client_cert = false

# ─── Action Signing & Verification ───────────────────────
[signing]
# enabled = false                    # Validate Ed25519 action signatures
# key_dir = "./keys"                 # Directory with public keys for verification

# ─── Attachments ─────────────────────────────────────────
[attachments]
# max_inline_bytes = 5242880         # 5 MB max per attachment
# max_attachments_per_action = 10    # Max attachments per action

# ─── Rate Limiting ───────────────────────────────────────
[rate_limit]
# enabled = false                    # Enable per-tenant sliding window rate limits
# config_path = "ratelimit.toml"    # Path to rate limit rules

# ─── Agentic Message Bus ─────────────────────────────────
[bus]
# enabled = false                    # Enable Kafka-backed agentic message bus
# [bus.kafka]
# bootstrap_servers = "localhost:9092"
# client_id = "acteon-bus"
# produce_timeout_ms = 5000
# transactional_id = "acteon-node-1" # Optional transactional producer
```

## Section Details

### `[server]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `"127.0.0.1"` | Bind address |
| `port` | u16 | `8080` | Bind port |
| `shutdown_timeout_seconds` | u64 | `30` | Max time to drain pending tasks on shutdown |
| `max_sse_connections_per_tenant` | usize | `10` | Max concurrent SSE stream connections per tenant |

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
| `dlq_enabled` | bool | `false` | Enable the dead-letter queue |
| `dlq_retention_seconds` | u64? | — | Remove action and A2A push DLQ entries at this age |

### `[auth]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable authentication |
| `config_path` | string | — | Path to auth configuration file |
| `watch` | bool | `true` | Hot-reload auth config on file changes |

See [Authentication](../api/authentication.md) for auth config file format.

### `[circuit_breaker]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable circuit breakers |
| `failure_threshold` | u32 | `5` | Consecutive failures before opening |
| `success_threshold` | u32 | `2` | Consecutive successes in `HalfOpen` to close |
| `recovery_timeout_seconds` | u64 | `60` | Seconds in `Open` before probing |

#### `[circuit_breaker.providers.<name>]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `failure_threshold` | u32 | No | Override default failure threshold |
| `success_threshold` | u32 | No | Override default success threshold |
| `recovery_timeout_seconds` | u64 | No | Override default recovery timeout |
| `fallback_provider` | string | No | Provider to reroute to when circuit is open |

Per-provider fields inherit from the defaults when not specified. The `fallback_provider` must reference a registered provider and cannot reference itself.

See [Circuit Breaker](../features/circuit-breaker.md) for feature documentation.

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

### `[telemetry]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable OpenTelemetry distributed tracing |
| `endpoint` | string | `"http://localhost:4317"` | OTLP exporter endpoint |
| `service_name` | string | `"acteon"` | Service name reported in traces |
| `sample_ratio` | f64 | `1.0` | Sampling ratio (`0.0` = none, `1.0` = all requests) |
| `protocol` | string | `"grpc"` | OTLP transport: `"grpc"` (port 4317) or `"http"` (port 4318) |
| `timeout_seconds` | u64 | `10` | Exporter timeout in seconds |

#### `[telemetry.resource_attributes]`

Arbitrary key-value pairs added to every exported span as OpenTelemetry resource attributes. Useful for tagging deployment environment, region, or instance ID.

See [Distributed Tracing](../features/distributed-tracing.md) for feature documentation.

### `[[providers]]`

Provider configuration. Multiple providers can be defined.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique provider name used in action dispatch |
| `type` | string | Yes | Provider type (see below) |

**Built-in provider types:**

| Type | Description | Extra Fields |
|------|-------------|-------------|
| `"log"` | Logs actions (no external calls) | — |
| `"webhook"` | HTTP webhook | `url`, `headers` |
| `"slack"` | Slack Web API / Webhook | `webhook_url`, `default_channel` |
| `"email"` | Email (SMTP or AWS SES) | `email_backend`, `from_address`, `smtp_host`, `smtp_port`, `username`, `password`, `tls`, `aws_region`, `ses_configuration_set` |
| `"twilio"` | Twilio SMS / MMS | `account_sid`, `auth_token`, `from_number` |
| `"teams"` | Microsoft Teams | `webhook_url` |
| `"discord"` | Discord Webhook | `webhook_url` |
| `"pagerduty"` | PagerDuty Events API v2 | `routing_key` |
| `"opsgenie"` | OpsGenie Alerts API | `opsgenie.api_key`, `opsgenie.region`, `opsgenie.default_team` |
| `"victorops"` | VictorOps / Splunk On-Call | `victorops.api_key`, `victorops.default_route`, `victorops.routes` |
| `"pushover"` | Pushover push notifications | `pushover.app_token`, `pushover.default_recipient`, `pushover.recipients` |
| `"telegram"` | Telegram Bot | `telegram.bot_token`, `telegram.default_chat`, `telegram.chats` |
| `"wechat"` | WeChat Work (企业微信) | `wechat.corp_id`, `wechat.corp_secret`, `wechat.agent_id`, `wechat.default_touser` |
| `"aws-sns"` | AWS Simple Notification Service | `aws_region`, `topic_arn` |
| `"aws-lambda"` | AWS Lambda function invocation | `aws_region`, `function_name`, `qualifier` |
| `"aws-eventbridge"` | AWS EventBridge custom bus | `aws_region`, `event_bus_name` |
| `"aws-sqs"` | AWS Simple Queue Service | `aws_region`, `queue_url` |
| `"aws-s3"` | AWS S3 object storage | `aws_region`, `bucket_name`, `object_prefix` |
| `"aws-ses"` | AWS Simple Email Service | `aws_region`, `from_address`, `ses_configuration_set` |
| `"aws-ec2"` | AWS EC2 lifecycle actions | `aws_region`, `default_security_group_ids`, `default_subnet_id`, `default_key_name` |
| `"aws-autoscaling"` | AWS Auto Scaling actions | `aws_region` |
| `"azure-blob"` | Azure Blob Storage | `azure_account_name`, `azure_container_name`, `azure_blob_prefix` |
| `"azure-eventhubs"` | Azure Event Hubs | `azure_namespace`, `azure_event_hub_name` |
| `"gcp-storage"` | Google Cloud Storage | `gcp_project_id`, `gcp_bucket`, `gcp_object_prefix` |
| `"gcp-pubsub"` | Google Cloud Pub/Sub | `gcp_project_id`, `gcp_topic` |
| `"swarm"` | Ambient agent swarm orchestrator | `swarm.config_path`, `swarm.hooks_binary`, `swarm.max_concurrent_runs` |
| `"governed-model"` | Content-addressed typed JSON inference | `model.endpoint`, `model.health_endpoint`, `model.lock_file`, `model.response_contract` |

**Common AWS fields** (all optional, shared across all `aws-*` types and `email` with `backend = "ses"`):

| Field | Type | Description |
|-------|------|-------------|
| `aws_region` | string | AWS region (required for AWS types) |
| `aws_endpoint_url` | string | Endpoint URL override (for LocalStack) |
| `aws_role_arn` | string | IAM role ARN to assume via STS |
| `aws_session_name` | string | STS session name (default: `"acteon-aws-provider"`) |
| `aws_external_id` | string | External ID for cross-account trust policies |

See [AWS Providers](../features/aws-providers.md), [Native Providers](../features/native-providers.md), and [Governed Model Provider](../features/governed-model-provider.md) for full payload format documentation.

### `[encryption]`

Payload encryption at rest using AES-256-GCM. Requires `ACTEON_PAYLOAD_KEY` to be set to a 32-byte key (hex or base64 encoded).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable payload encryption for state and audit backends |

### `[quotas]`

Tenant quota policy enforcement.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable quota enforcement |
| `default_window` | string | `"daily"` | Default reset window (`"hourly"`, `"daily"`, `"monthly"`) |
| `default_overage_behavior` | string | `"block"` | Overage behavior (`"block"`, `"drop"`, `"warn"`) |
| `policies_file` | string | — | Path to static TOML quota policies file |
| `watch` | bool | `true` | Hot-reload policies file on changes |

### `[templates]`

MiniJinja payload templates and mapping profiles.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `manifest_file` | string | — | Path to TOML template/profile manifest file |
| `watch` | bool | `true` | Hot-reload template manifest on file changes |

### `[compliance]`

Compliance mode presets and cryptographically chained audit logging for SOC2 / HIPAA.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"none"` | Compliance preset: `"none"`, `"soc2"`, or `"hipaa"` |
| `sync_audit_writes` | bool | preset | Require synchronous audit writes before dispatch completion |
| `immutable_audit` | bool | preset | Disallow record deletion / updates |
| `hash_chain` | bool | preset | Enable SHA-256 hash chaining of audit records |

### `[wasm]`

WASM plugin runtime for sandboxed custom rule evaluations and transformations.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable WASM runtime |
| `plugin_dir` | string | — | Directory to scan for `.wasm` plugins |
| `default_memory_limit_bytes` | u64 | `16777216` | Memory limit per plugin (16 MB) |
| `default_timeout_ms` | u64 | `100` | Per-execution timeout in milliseconds |

### `[tls]`

TLS termination for inbound HTTP and mutual TLS (mTLS) client certificate verification.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable TLS |
| `cert_path` | string | — | Path to server X.509 certificate PEM |
| `key_path` | string | — | Path to server private key PEM |
| `client_ca_path` | string | — | Path to client CA certificate PEM for mTLS |
| `require_client_cert` | bool | `false` | Reject connections without valid client certificate |

### `[signing]`

Ed25519 cryptographic action signing and verification.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enforce signature verification on inbound actions |
| `key_dir` | string | — | Directory containing authorized public key files |

### `[attachments]`

Controls constraints on action payload attachments.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_inline_bytes` | u64 | `5242880` | Maximum size in bytes per attachment (5 MB) |
| `max_attachments_per_action` | usize | `10` | Maximum attachments allowed per action |

### `[bus]`

Agentic message bus backed by Apache Kafka. Requires compiling with `--features bus`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable the message bus subsystem |

#### `[bus.kafka]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bootstrap_servers` | string | `"localhost:9092"` | Kafka broker bootstrap list |
| `client_id` | string | `"acteon-bus"` | Client ID for Kafka connections |
| `produce_timeout_ms` | u64 | `5000` | Produce timeout in milliseconds |
| `transactional_id` | string | — | Optional transactional ID for exactly-once producing |
| `transaction_timeout_ms` | u64 | `60000` | Transaction timeout in milliseconds |

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
| `examples/clickhouse.toml` | ClickHouse audit |
| `examples/elasticsearch-audit.toml` | Redis state + Elasticsearch audit |
| `examples/dynamodb.toml` | DynamoDB state backend |
| `examples/full.toml` | All options documented |
| `examples/aws-event-pipeline/acteon.toml` | AWS providers (SNS, Lambda, EventBridge, SQS) + DynamoDB |
| `examples/agent-swarm-coordination/acteon.toml` | Claude Code agent governance with PostgreSQL |

```bash
# Start with Redis
docker compose up -d
cargo run -p acteon-server -- -c examples/redis.toml

# Start with PostgreSQL
docker compose --profile postgres up -d
scripts/migrate.sh -c examples/postgres.toml
cargo run -p acteon-server --features postgres -- -c examples/postgres.toml
```

## Replicated authentication configuration

Optional `[auth.authority]` uses the configured state backend to coordinate an explicit
`authority_revision` in `auth.toml`. It publishes configuration before installing
local tables and refuses authentication from stale replicas. Configure a dedicated
control scope and shared fingerprint key. The scope is permanently reserved for
that source; normal startup connects to existing compatible state. See [shared authentication authority](../api/authentication.md#shared-authentication-authority-across-replicas)
for bootstrap, reload and recovery semantics. This setting governs authentication
freshness; it does not enable per-effect execution permits.
