<p align="center">
  <img src="docs/logo.png" alt="Acteon" width="200">
</p>

<h1 align="center">acteon</h1>

<p align="center">Actions forged in Rust</p>

<p align="center">
  <a href="https://github.com/penserai/acteon/actions/workflows/ci.yml"><img src="https://github.com/penserai/acteon/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

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
| [`acteon-client`](acteon-client/README.md) | Native Rust HTTP client for the Acteon API |
| [`acteon-simulation`](acteon-simulation/README.md) | Testing framework with mock providers and failure injection |

## Running locally

### Prerequisites

- Rust 1.88+
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
# shutdown_timeout_seconds = 30  # Max time to wait for pending tasks during shutdown

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

[auth]
# enabled = false
# config_path = "auth.toml"   # Path to auth config file
# watch = true                # Hot-reload on file changes

[audit.redact]
# enabled = false
# fields = ["password", "token", "api_key", "secret"]
# placeholder = "[REDACTED]"
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

## Lock consistency

Acteon uses distributed locks to ensure only one instance processes a given action at a time. The consistency guarantees vary by backend:

| Backend | Failover Behavior | Recommendation |
|---------|-------------------|----------------|
| Redis (single) | Strong mutual exclusion | Good for development, single-node production |
| Redis (Sentinel/Cluster) | Lock may be lost during failover | Use only if occasional duplicates are acceptable |
| PostgreSQL | Locks survive failover (ACID) | Recommended for strong consistency |
| DynamoDB | Strong consistency available | Recommended for strong consistency |
| Memory | Single-process only | Development/testing only |

If your application requires strict mutual exclusion guarantees (e.g., financial transactions), use PostgreSQL or DynamoDB as your state backend. The Redis backend is suitable for scenarios where occasional duplicate processing during rare failover events is acceptable.

## Testing & Simulation

The `acteon-simulation` crate provides comprehensive testing tools:

```rust
use acteon_simulation::prelude::*;
use acteon_core::Action;

#[tokio::test]
async fn test_deduplication() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE)
            .build()
    ).await.unwrap();

    let action = Action::new("ns", "tenant", "email", "notify", json!({}))
        .with_dedup_key("unique-key");

    harness.dispatch(&action).await.unwrap().assert_executed();
    harness.dispatch(&action).await.unwrap().assert_deduplicated();

    harness.provider("email").unwrap().assert_called(1);
    harness.teardown().await.unwrap();
}
```

### Features

- **RecordingProvider**: Captures all provider calls for verification
- **FailingProvider**: Simulates timeouts, connection errors, rate limiting
- **Mixed Backends**: Test any combination of state and audit backends
- **Failure Injection**: `FailureMode::EveryN`, `FirstN`, `Probabilistic`
- **End-to-End Audit**: Verify all outcomes are recorded (executed, suppressed, deduplicated, throttled, failed)

### Running Simulations

```sh
# Single backend simulations
cargo run -p acteon-simulation --example redis_simulation --features redis
cargo run -p acteon-simulation --example postgres_simulation --features postgres

# Mixed backend simulations (e.g., Redis state + PostgreSQL audit)
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "redis,postgres" -- redis-postgres
```

See the [acteon-simulation README](acteon-simulation/README.md) for full documentation.

## Client Libraries

### Rust

The `acteon-client` crate provides a native Rust HTTP client:

```rust
use acteon_client::ActeonClient;
use acteon_core::Action;

let client = ActeonClient::new("http://localhost:8080");

// Check health
assert!(client.health().await?);

// Dispatch an action
let action = Action::new("ns", "tenant", "email", "send", json!({"to": "user@example.com"}));
let outcome = client.dispatch(&action).await?;

// Query audit trail
let audit = client.query_audit(&AuditQuery {
    tenant: Some("tenant".to_string()),
    limit: Some(10),
    ..Default::default()
}).await?;
```

See the [acteon-client README](acteon-client/README.md) for full documentation.

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
