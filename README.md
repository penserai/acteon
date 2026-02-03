<p align="center">
  <img src="docs/logo.png" alt="Acteon" width="200">
</p>

<h1 align="center">acteon</h1>

<p align="center">Actions forged in Rust</p>

Acteon is an action gateway that dispatches actions through a configurable pipeline of rules, providers, and state backends.

The name draws from the Greek myth of Actaeon, a hunter transformed by Artemis into a stag -- the very thing he pursued. Likewise, actions entering Acteon are transformed -- deduplicated, rerouted, throttled, or dispatched -- before they ever reach the outside world.

## Architecture

| Crate | Description |
|-------|-------------|
| `acteon-core` | Shared types (`Action`, `ActionOutcome`, newtypes) |
| `acteon-state` | Abstract state store / distributed lock trait |
| `acteon-state-memory` | In-memory state backend |
| `acteon-state-redis` | Redis state backend |
| `acteon-state-postgres` | PostgreSQL state backend |
| `acteon-state-dynamodb` | DynamoDB state backend |
| `acteon-state-clickhouse` | ClickHouse state backend |
| `acteon-audit` | Abstract audit trail trait |
| `acteon-audit-memory` | In-memory audit backend |
| `acteon-audit-postgres` | PostgreSQL audit backend |
| `acteon-audit-clickhouse` | ClickHouse audit backend |
| `acteon-audit-elasticsearch` | Elasticsearch audit backend |
| `acteon-rules` | Rule engine IR and evaluation |
| `acteon-rules-yaml` | YAML rule file parser |
| `acteon-rules-cel` | CEL expression support |
| `acteon-provider` | Provider trait and registry |
| `acteon-executor` | Action execution with retries and concurrency |
| `acteon-gateway` | Orchestrates lock, rules, execution |
| `acteon-server` | HTTP server (Axum) with Swagger UI |
| `acteon-email` | Email/SMTP provider |
| `acteon-slack` | Slack provider |

## Running locally

### Prerequisites

- Rust 1.85+
- Cargo

### Quick start (in-memory, no config file needed)

```sh
cargo run -p acteon-server
```

The server starts on `http://127.0.0.1:8080` with the in-memory state backend and no rules loaded. You can then:

- Open **Swagger UI** at [http://127.0.0.1:8080/swagger-ui/](http://127.0.0.1:8080/swagger-ui/)
- Fetch the **OpenAPI spec** at [http://127.0.0.1:8080/api-doc/openapi.json](http://127.0.0.1:8080/api-doc/openapi.json)
- Hit the **health endpoint**: `curl http://127.0.0.1:8080/health`

### CLI options

```
cargo run -p acteon-server -- [OPTIONS]

Options:
  -c, --config <PATH>   Path to TOML config file [default: acteon.toml]
      --host <HOST>      Override bind host
      --port <PORT>      Override bind port
```

Examples:

```sh
# Custom port
cargo run -p acteon-server -- --port 3000

# With a config file
cargo run -p acteon-server -- -c my-config.toml
```

### Configuration

Create an `acteon.toml` file (all sections are optional -- defaults are shown):

```toml
[server]
host = "127.0.0.1"
port = 8080

[state]
backend = "memory"   # "memory", "redis", "postgres", "dynamodb", or "clickhouse"
# url = "redis://localhost:6379"
# prefix = "acteon"
# region = "us-east-1"       # DynamoDB only
# table_name = "acteon"      # DynamoDB only

[audit]
# enabled = false
# backend = "memory"         # "memory", "postgres", "clickhouse", or "elasticsearch"
# url = "postgres://acteon:acteon@localhost:5432/acteon"
# prefix = "acteon_"
# ttl_seconds = 2592000      # 30 days
# cleanup_interval_seconds = 3600
# store_payload = true

[rules]
# directory = "./rules"      # Path to YAML rule files

[executor]
# max_retries = 3
# timeout_seconds = 30
# max_concurrent = 100
```

### Environment

Set the `RUST_LOG` environment variable to control log verbosity:

```sh
RUST_LOG=debug cargo run -p acteon-server
```

## Development with backends

The `docker-compose.yml` ships with profiles for every supported backend. Redis runs by default; all others are opt-in.

### Available backends

| Backend | Type | Docker profile | Default URL |
|---------|------|----------------|-------------|
| Memory | state, audit | *(none)* | n/a |
| Redis | state | *(default)* | `redis://localhost:6379` |
| PostgreSQL | state, audit | `postgres` | `postgres://acteon:acteon@localhost:5432/acteon` |
| ClickHouse | state, audit | `clickhouse` | `http://localhost:8123` |
| Elasticsearch | audit | `elasticsearch` | `http://localhost:9200` |
| DynamoDB Local | state | `dynamodb` | `http://localhost:8000` |

### Starting backends

```sh
# Start Redis (default, always runs)
docker compose up -d

# Start a single optional backend
docker compose --profile postgres up -d

# Start multiple backends at once
docker compose --profile postgres --profile elasticsearch up -d
```

### Example configurations

Ready-to-use config files are provided in the `examples/` directory. Pair each one with the matching Docker profile:

```sh
# Redis state (default Docker services)
docker compose up -d
cargo run -p acteon-server -- -c examples/redis.toml

# PostgreSQL state + audit
docker compose --profile postgres up -d
cargo run -p acteon-server -- -c examples/postgres.toml

# ClickHouse state + audit
docker compose --profile clickhouse up -d
cargo run -p acteon-server -- -c examples/clickhouse.toml

# Redis state + Elasticsearch audit
docker compose --profile elasticsearch up -d
cargo run -p acteon-server -- -c examples/elasticsearch-audit.toml

# DynamoDB Local state
docker compose --profile dynamodb up -d
cargo run -p acteon-server -- -c examples/dynamodb.toml
```

### Combining backends

State and audit backends are independent. You can mix any state backend with any audit backend:

```toml
# Redis for state, PostgreSQL for audit
[state]
backend = "redis"
url = "redis://localhost:6379"

[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"
```

```sh
docker compose --profile postgres up -d
cargo run -p acteon-server -- -c acteon.toml
```

## API endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check with metrics snapshot |
| GET | `/metrics` | Dispatch counters |
| POST | `/v1/dispatch` | Dispatch a single action |
| POST | `/v1/dispatch/batch` | Dispatch multiple actions |
| GET | `/v1/rules` | List loaded rules |
| POST | `/v1/rules/reload` | Reload rules from a directory |
| PUT | `/v1/rules/{name}/enabled` | Enable or disable a rule |
| GET | `/v1/audit` | Query audit records with filters |
| GET | `/v1/audit/{action_id}` | Get audit record by action ID |

Full request/response schemas are available in the Swagger UI.

## Tests

```sh
cargo test --workspace
```

## Linting

```sh
cargo clippy --workspace --no-deps -- -D warnings
cargo fmt --all -- --check
```

## License

Copyright 2026 Penserai Inc.

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
